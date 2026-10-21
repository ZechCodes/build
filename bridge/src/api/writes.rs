//! Durable write envelopes shared by RPC admission, status and confirmations.
//!
//! No semantic method is admitted yet. Request metadata carries identity and
//! dependencies, never a caller-selected protocol mode or authenticated scope.

use serde::{de::Error, Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// Internal rollout policy; this is deliberately absent from capabilities.
pub const ACK_CONFIRM_METHODS: &[&str] = &[];
pub const MAX_DEPENDENCIES: usize = 8;
pub const MAX_STATUS_OPERATIONS: usize = 128;
pub const MAX_STATUS_BYTES: usize = 512 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WriteMetadata {
    #[serde(deserialize_with = "operation_id")]
    pub operation_id: String,
    /// Omitted means infer creators. Explicit empty is an override.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "dependencies"
    )]
    pub after: Option<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct StatusParams {
    #[serde(deserialize_with = "operation_set")]
    pub operation_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct SubscribeParams {
    #[serde(deserialize_with = "operation_set")]
    pub operation_ids: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct OwnerSequence {
    pub kind: String,
    pub id: String,
    /// Sequences restart only within a distinct bound owner generation.
    pub generation: u64,
    pub sequence: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct RefreshAddress {
    pub kind: String,
    pub id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AckPhase {
    Ack,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AcceptedState {
    Accepted,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct WriteAck {
    pub phase: AckPhase,
    #[serde(deserialize_with = "operation_id")]
    pub operation_id: String,
    pub revision: u64,
    pub state: AcceptedState,
    pub accepted_epoch: String,
    pub owners: Vec<OwnerSequence>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct AckReply<R = Value> {
    pub id: Value,
    #[serde(deserialize_with = "true_only")]
    pub ok: bool,
    pub bridge_epoch: String,
    pub write: WriteAck,
    /// Per-method acceptance data, with typed overlays and reserved IDs.
    pub result: R,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EffectOutcome {
    Unapplied,
    Partial,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct WriteError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TerminalState {
    Succeeded,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ConfirmationOutcome<R = Value> {
    Succeeded {
        result: R,
        published: Vec<OwnerSequence>,
        refresh: Vec<RefreshAddress>,
    },
    Failed {
        effect: EffectOutcome,
        error: WriteError,
        refresh: Vec<RefreshAddress>,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum ConfirmedEvent {
    #[serde(rename = "writes.confirmed")]
    Confirmed,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct WriteConfirmation<R = Value> {
    #[serde(rename = "type")]
    pub event_type: ConfirmedEvent,
    pub bridge_epoch: String,
    #[serde(deserialize_with = "operation_id")]
    pub operation_id: String,
    pub revision: u64,
    #[serde(flatten)]
    pub outcome: ConfirmationOutcome<R>,
}

/// The saved acknowledgement is returned intact so a reconnect can restore
/// accepted cache effects without synthesizing them from current domain state.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum OperationSummary<R = Value> {
    Accepted {
        operation_id: String,
        revision: u64,
        ack: WriteAck,
        result: R,
    },
    Executing {
        operation_id: String,
        revision: u64,
        ack: WriteAck,
        result: R,
    },
    Succeeded {
        operation_id: String,
        revision: u64,
        result: R,
        published: Vec<OwnerSequence>,
        refresh: Vec<RefreshAddress>,
    },
    Failed {
        operation_id: String,
        revision: u64,
        effect: EffectOutcome,
        error: WriteError,
        refresh: Vec<RefreshAddress>,
    },
    NotFound {
        operation_id: String,
    },
    ResultExpired {
        operation_id: String,
        revision: u64,
        outcome: TerminalState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effect: Option<EffectOutcome>,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct StatusResult<R = Value> {
    pub journal_incarnation: String,
    pub operations: Vec<OperationSummary<R>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct SubscribeResult<R = Value> {
    pub journal_incarnation: String,
    /// Decimal u64, avoiding loss of precision in JavaScript clients.
    #[serde(deserialize_with = "decimal_cursor")]
    pub cursor: String,
    pub operations: Vec<OperationSummary<R>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

fn valid_operation_id(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| id.hyphenated().to_string() == value)
}

fn operation_id<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let value = String::deserialize(deserializer)?;
    if !valid_operation_id(&value) {
        return Err(D::Error::custom("operation_id must be a canonical UUID"));
    }
    Ok(value)
}

fn check_ids<E: Error>(ids: &[String], maximum: usize) -> Result<(), E> {
    if ids.len() > maximum {
        return Err(E::custom(format!(
            "at most {maximum} operation IDs are allowed"
        )));
    }
    if !ids.iter().all(|id| valid_operation_id(id)) {
        return Err(E::custom("operation IDs must be canonical UUIDs"));
    }
    Ok(())
}

fn dependencies<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Vec<String>>, D::Error> {
    let ids = Option::<Vec<String>>::deserialize(deserializer)?;
    // Null cannot silently change an explicit override into inference.
    let ids = ids.ok_or_else(|| D::Error::custom("after must be an array when supplied"))?;
    check_ids::<D::Error>(&ids, MAX_DEPENDENCIES)?;
    Ok(Some(ids))
}

fn operation_set<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<String>, D::Error> {
    let ids = Vec::<String>::deserialize(deserializer)?;
    if ids.is_empty() {
        return Err(D::Error::custom("operation_ids must not be empty"));
    }
    check_ids::<D::Error>(&ids, MAX_STATUS_OPERATIONS)?;
    Ok(ids)
}

fn decimal_cursor<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let value = String::deserialize(deserializer)?;
    if value.parse::<u64>().is_err() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(D::Error::custom("cursor must be a decimal u64 string"));
    }
    Ok(value)
}

fn true_only<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    if !bool::deserialize(deserializer)? {
        return Err(D::Error::custom("an acknowledgement must be successful"));
    }
    Ok(true)
}
