//! Configured compaction intent. These values do not apply a harness setting;
//! session opening and enforcement belong to the harness layer.

use crate::models::AgentProvider;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub const MAX_SAFE_TOKENS: u64 = 9_007_199_254_740_991;

pub fn validate_tokens(tokens: u64) -> Result<u64, String> {
    if (1..=MAX_SAFE_TOKENS).contains(&tokens) {
        Ok(tokens)
    } else {
        Err("compaction threshold must be a positive safe integer of tokens".into())
    }
}

pub fn parse_tokens(value: &Value) -> Result<u64, String> {
    validate_tokens(
        value
            .as_u64()
            .ok_or("compaction threshold must be a positive safe integer of tokens")?,
    )
}

/// A sparse map. Only config loading can retain an integer outside the safe
/// wire range; mutations cannot create one. Disk values remain exact u64s.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CompactionThresholds(BTreeMap<String, u64>);

impl CompactionThresholds {
    pub fn get(&self, provider: AgentProvider) -> Option<u64> {
        self.0.get(provider.wire_id()).copied()
    }

    pub(crate) fn broadcast(tokens: u64) -> Self {
        Self(
            AgentProvider::ALL
                .into_iter()
                .map(|p| (p.wire_id().into(), tokens))
                .collect(),
        )
    }

    pub(crate) fn from_config(value: &Value) -> Result<Self, String> {
        let mut thresholds = Self::default();
        if value.is_null() {
            return Ok(thresholds);
        }
        for (provider, value) in provider_object(value)? {
            if value.is_null() {
                continue;
            }
            let tokens = value
                .as_u64()
                .filter(|tokens| *tokens > 0)
                .ok_or("stored compaction threshold must be a positive integer")?;
            thresholds.0.insert(provider.clone(), tokens);
        }
        Ok(thresholds)
    }

    pub fn patched(&self, patch: &Value) -> Result<Self, String> {
        let mut next = self.clone();
        for (provider, value) in provider_object(patch)? {
            if value.is_null() {
                next.0.remove(provider);
            } else {
                next.0.insert(provider.clone(), parse_tokens(value)?);
            }
        }
        Ok(next)
    }

    /// For the bridge-owned config only, never a new client read field.
    pub fn stored_value(&self) -> Value {
        json!(self.0)
    }

    /// A safe read representation, including exact legacy values that cannot
    /// be represented by a JavaScript number. It is not a mutation shape.
    pub fn read_value(&self) -> Value {
        Value::Object(
            self.0
                .iter()
                .map(|(provider, tokens)| {
                    let value = if *tokens <= MAX_SAFE_TOKENS {
                        json!(tokens)
                    } else {
                        json!({"tokens_decimal": tokens.to_string()})
                    };
                    (provider.clone(), value)
                })
                .collect(),
        )
    }
}

fn provider_object(value: &Value) -> Result<&serde_json::Map<String, Value>, String> {
    let object = value
        .as_object()
        .ok_or("compaction thresholds must be a provider map")?;
    for provider in object.keys() {
        if AgentProvider::from_wire(provider).is_none() {
            return Err(format!("unknown compaction provider: {provider}"));
        }
    }
    Ok(object)
}

/// A mutation has exactly one scope. The patch itself contains only provider
/// keys, so mixing scopes or legacy scalar fields is rejected atomically.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompactionScope {
    Device,
    Project(String),
    Workspace(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum CompactionSource {
    Default,
    Device,
    Project(String),
    Workspace(String),
    Conversation(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedCompactionPolicy {
    pub tokens: u64,
    pub source: CompactionSource,
}

impl ResolvedCompactionPolicy {
    pub fn observation(&self) -> Value {
        if self.tokens <= MAX_SAFE_TOKENS {
            json!({"tokens": self.tokens, "source": self.source})
        } else {
            json!({"tokens": null, "tokens_decimal": self.tokens.to_string(),
                "source": self.source, "reason": "unsupported_representation"})
        }
    }
}

/// Old conversation zeros meant never; the current policy has only positive
/// overrides and inheritance. Normalize at every agent JSON load boundary.
pub(crate) fn deserialize_override<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<u64>, D::Error> {
    Ok(Option::<u64>::deserialize(deserializer)?.filter(|tokens| *tokens > 0))
}
