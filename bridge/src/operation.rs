//! Durable receipts for reviewer operations that cross a provider boundary.
//!
//! A receipt is committed with the transcript mutation it acknowledges.  Its
//! delivery intent is then claimed durably before any provider-facing write.
//! That ordering makes restart recovery conservative: queued work is safe to
//! replay, while a claim left behind by a dead process is explicitly uncertain.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::models::ModelChoice;
use crate::thread::ThreadMessage;

pub const THREAD_POST_METHOD: &str = "thread.post";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OperationStatus {
    Queued,
    Claimed,
    Delivered,
    Uncertain,
}

impl OperationStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Claimed => "claimed",
            Self::Delivered => "delivered",
            Self::Uncertain => "uncertain",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryIntent {
    pub root: PathBuf,
    pub owner_id: String,
    pub agent_id: String,
    pub model_choice: ModelChoice,
    pub choice_revision: u64,
    pub interrupt: bool,
    /// Immutable conversation slice this provider turn is allowed to consume.
    /// `None` only decodes short-lived pre-fence development receipts; recovery
    /// treats those as ambiguous instead of issuing an unrestricted read.
    #[serde(default)]
    pub payload: Option<OperationPayload>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationPayload {
    pub start_sequence: u64,
    pub end_sequence: u64,
    pub messages: Vec<ThreadMessage>,
    /// Bounded conversation context as it stood immediately before acceptance.
    pub prior_context: String,
}

impl OperationPayload {
    pub fn delivery_prompt(&self, operation_id: &str, cold: bool) -> String {
        let messages = serde_json::to_string_pretty(&self.messages)
            .expect("operation messages always serialize");
        let context = if cold && !self.prior_context.is_empty() {
            format!(
                "\nConversation context before this operation:\n{}\n",
                self.prior_context
            )
        } else {
            String::new()
        };
        format!(
            "Process only reviewer operation `{operation_id}` (conversation sequences {} through {}).\
             {context}\nExact accepted messages:\n{messages}\n\
             Call `read_unread_messages` with `operation_id` set to `{operation_id}` to acknowledge exactly these messages; do not consume another operation's messages.",
            self.start_sequence, self.end_sequence
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationReceipt {
    pub operation_id: String,
    pub method: String,
    pub entity_id: String,
    pub agent_id: String,
    pub conversation_id: String,
    pub choice_revision: u64,
    pub posted_sequence: u64,
    pub message_start_sequence: u64,
    pub status: OperationStatus,
    /// A known terminal delivery failure. Absence means either delivery is
    /// still pending/ambiguous or the provider handoff completed normally.
    #[serde(default)]
    pub execution_error: Option<String>,
    pub request_hash: String,
    pub delivery: Option<DeliveryIntent>,
}

impl OperationReceipt {
    pub fn wire_value(&self) -> Value {
        serde_json::json!({
            "operation_id": self.operation_id,
            "method": self.method,
            "entity_id": self.entity_id,
            "agent_id": self.agent_id,
            "conversation_id": self.conversation_id,
            "choice_revision": self.choice_revision,
            "posted_sequence": self.posted_sequence,
            "message_start_sequence": self.message_start_sequence,
            "status": self.status,
            "operation_error": self.execution_error,
        })
    }

    pub fn matches_request(&self, method: &str, request_hash: &str) -> bool {
        self.method == method && self.request_hash == request_hash
    }
}

/// Hash a request independently of JSON object insertion order. The operation
/// id itself is excluded: it names the receipt and is not part of its payload.
pub fn request_hash(method: &str, params: &Value) -> String {
    let mut params = params.clone();
    if let Some(object) = params.as_object_mut() {
        object.remove("operation_id");
        object.remove("thread_limit");
        object.remove("thread_after_sequence");
        object.remove("before_sequence");
    }
    let normalized = normalize_json(params);
    let envelope = serde_json::json!({ "method": method, "params": normalized });
    format!("{:x}", Sha256::digest(envelope.to_string().as_bytes()))
}

/// Bind a post's semantic payload to the address the bridge validated. Read
/// projection fields are deliberately absent, so changing the returned page on
/// retry does not turn the same send into an operation-id conflict.
pub fn thread_post_request_hash(
    params: &Value,
    entity_id: &str,
    agent_id: &str,
    conversation_id: &str,
) -> String {
    let payload_hash = request_hash(THREAD_POST_METHOD, params);
    let envelope = serde_json::json!({
        "payload_hash": payload_hash,
        "entity_id": entity_id,
        "agent_id": agent_id,
        "conversation_id": conversation_id,
    });
    format!("{:x}", Sha256::digest(envelope.to_string().as_bytes()))
}

fn normalize_json(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(normalize_json).collect()),
        Value::Object(values) => {
            let mut keys = values.keys().cloned().collect::<Vec<_>>();
            keys.sort();
            let mut normalized = Map::new();
            for key in keys {
                let value = values
                    .get(&key)
                    .expect("the key came from this object")
                    .clone();
                normalized.insert(key, normalize_json(value));
            }
            Value::Object(normalized)
        }
        scalar => scalar,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_hash_ignores_object_order_and_operation_id() {
        let left = serde_json::json!({
            "operation_id": "first",
            "entity_id": "issue-1",
            "body": "hello",
            "anchor": { "line": 3, "path": "src/lib.rs" }
        });
        let right = serde_json::json!({
            "anchor": { "path": "src/lib.rs", "line": 3 },
            "body": "hello",
            "entity_id": "issue-1",
            "operation_id": "second"
        });
        assert_eq!(
            request_hash(THREAD_POST_METHOD, &left),
            request_hash(THREAD_POST_METHOD, &right)
        );
    }

    #[test]
    fn thread_post_hash_ignores_read_projection_but_binds_resolved_address() {
        let first = serde_json::json!({
            "entity_id": "issue-1",
            "body": "hello",
            "thread_limit": 20
        });
        let retry = serde_json::json!({
            "entity_id": "issue-1",
            "body": "hello",
            "thread_after_sequence": 9
        });
        let hash = thread_post_request_hash(&first, "issue-1", "agent-1", "conversation-1");
        assert_eq!(
            hash,
            thread_post_request_hash(&retry, "issue-1", "agent-1", "conversation-1",)
        );
        assert_ne!(
            hash,
            thread_post_request_hash(&retry, "issue-1", "agent-2", "conversation-1",)
        );
    }

    #[test]
    fn thread_post_hash_binds_viewing_context_and_keeps_legacy_hash_stable() {
        let legacy = serde_json::json!({ "entity_id": "issue-1", "body": "hello" });
        let expected = thread_post_request_hash(&legacy, "issue-1", "agent-1", "conversation-1");
        assert_eq!(
            expected,
            thread_post_request_hash(&legacy, "issue-1", "agent-1", "conversation-1")
        );

        let contextual = serde_json::json!({
            "entity_id": "issue-1",
            "body": "hello",
            "viewing_context": { "version": 1, "items": [{ "kind": "file", "path": "src/lib.rs" }] }
        });
        assert_ne!(
            expected,
            thread_post_request_hash(&contextual, "issue-1", "agent-1", "conversation-1")
        );
    }

    #[test]
    fn shared_browser_contract_matches_receipt_vocabulary() {
        let fixture: Value =
            serde_json::from_str(include_str!("../../fixtures/api/v1/thread.post.json")).unwrap();
        let contract = &fixture["operations"];
        assert_eq!(contract["post_method"], THREAD_POST_METHOD);
        assert_eq!(contract["status_method"], "thread.operation");
        assert_eq!(contract["operation_id"]["max_bytes"], 128);
        assert_eq!(
            contract["statuses"],
            serde_json::json!(["queued", "claimed", "delivered", "uncertain"])
        );
    }
}
