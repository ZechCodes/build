//! Opt-in conversation history policy. Scheduling lives in app/runtime/retention.
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RetentionConfig {
    pub enabled: bool,
    pub max_messages: usize,
    pub max_age_days: u64,
}

impl Default for RetentionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_messages: 100,
            max_age_days: 7,
        }
    }
}

impl RetentionConfig {
    /// Missing configuration opts out; any explicit malformed value stops boot.
    pub fn from_config(config: &Value) -> Result<Self, String> {
        let Some(value) = config.get("retention") else {
            return Ok(Self::default());
        };
        let policy: Self =
            serde_json::from_value(value.clone()).map_err(|e| format!("retention: {e}"))?;
        if policy.max_messages == 0 || i64::try_from(policy.max_messages).is_err() {
            return Err("retention.max_messages must be a positive SQLite integer".into());
        }
        let duration = i64::try_from(policy.max_age_days)
            .ok()
            .and_then(|days| days.checked_mul(86400))
            .map(time::Duration::seconds);
        if policy.max_age_days == 0
            || duration
                .and_then(|days| time::OffsetDateTime::now_utc().checked_sub(days))
                .is_none()
        {
            return Err(
                "retention.max_age_days must be positive and within the supported date range"
                    .into(),
            );
        }
        Ok(policy)
    }

    pub fn policy(self) -> crate::store::RetentionPolicy {
        crate::store::RetentionPolicy {
            max_messages: self.max_messages,
            max_age_days: self.max_age_days,
        }
    }
}

#[cfg(test)]
#[path = "retention/tests.rs"]
mod tests;
