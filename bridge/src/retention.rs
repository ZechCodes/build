//! Conversation history policy with an age window and message count floor. Scheduling lives in app/runtime/retention.
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RetentionConfig {
    pub enabled: bool,
    pub min_messages: usize,
    pub max_messages: usize,
    pub window_days: u64,
}

impl Default for RetentionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_messages: 50,
            max_messages: 100,
            window_days: 7,
        }
    }
}

impl RetentionConfig {
    /// Missing configuration uses the default policy; malformed values stop boot.
    pub fn from_config(config: &Value) -> Result<Self, String> {
        let Some(value) = config.get("retention") else {
            return Ok(Self::default());
        };
        let policy: Self =
            serde_json::from_value(value.clone()).map_err(|e| format!("retention: {e}"))?;
        if policy.max_messages == 0 || i64::try_from(policy.max_messages).is_err() {
            return Err("retention.max_messages must be a positive SQLite integer".into());
        }
        if policy.min_messages == 0 || policy.min_messages > policy.max_messages {
            return Err("retention.min_messages must be positive and at most max_messages".into());
        }
        let duration = i64::try_from(policy.window_days)
            .ok()
            .and_then(|days| days.checked_mul(86400))
            .map(time::Duration::seconds);
        if policy.window_days == 0
            || duration
                .and_then(|days| time::OffsetDateTime::now_utc().checked_sub(days))
                .is_none()
        {
            return Err(
                "retention.window_days must be positive and within the supported date range".into(),
            );
        }
        Ok(policy)
    }

    pub fn policy(self) -> crate::store::RetentionPolicy {
        crate::store::RetentionPolicy {
            min_messages: self.min_messages,
            max_messages: self.max_messages,
            max_age_days: self.window_days,
        }
    }
}

#[cfg(test)]
#[path = "retention/tests.rs"]
mod tests;
