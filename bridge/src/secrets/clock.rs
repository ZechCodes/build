use super::{Result, SecretError, SecretRecord, STAGED_LIFETIME_SECONDS};
use crate::store::StoreWriter;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// UTC is persisted; monotonic time only orders deadlines within this process.
pub trait Clock: Send + Sync {
    fn wall_seconds(&self) -> i64;
    fn monotonic_seconds(&self) -> u64;
}

pub(super) struct SystemClock(Instant);
impl Default for SystemClock {
    fn default() -> Self {
        Self(Instant::now())
    }
}
impl Clock for SystemClock {
    fn wall_seconds(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|time| i64::try_from(time.as_secs()).ok())
            .unwrap_or(-1)
    }
    fn monotonic_seconds(&self) -> u64 {
        self.0.elapsed().as_secs()
    }
}

pub(super) struct Observation {
    pub now: i64,
    pub effective_now: i64,
    pub monotonic: u64,
    pub rollback: bool,
}

struct Deadline {
    absolute: i64,
    attached_at: Option<i64>,
    monotonic: u64,
}

pub(super) struct ClockState {
    clock: Arc<dyn Clock>,
    highwater: i64,
    deadlines: HashMap<String, Deadline>,
}

impl ClockState {
    pub fn new(clock: Arc<dyn Clock>, highwater: i64) -> Self {
        Self {
            clock,
            highwater,
            deadlines: HashMap::new(),
        }
    }

    /// A FULL-sync metadata write precedes every read and physical deletion.
    pub fn observe(&mut self, writer: &StoreWriter) -> Result<Observation> {
        let now = self.clock.wall_seconds();
        if now < 0 {
            return Err(SecretError::Unavailable);
        }
        let rollback = !writer.secret_clock_observe(now)?;
        self.highwater = self.highwater.max(writer.secret_clock_highwater()?);
        Ok(Observation {
            now,
            effective_now: self.highwater,
            monotonic: self.clock.monotonic_seconds(),
            rollback,
        })
    }

    pub fn expired(&mut self, record: &SecretRecord, observation: &Observation) -> bool {
        let absolute = match record.attached_at {
            None => Some(record.created_at.saturating_add(STAGED_LIFETIME_SECONDS)),
            Some(_) => record.expires_at,
        };
        let Some(absolute) = absolute else {
            return false;
        };
        let remaining = absolute.saturating_sub(observation.effective_now).max(0) as u64;
        let observed = observation.monotonic.saturating_add(remaining);
        let deadline = self
            .deadlines
            .entry(record.context.id.clone())
            .or_insert(Deadline {
                absolute,
                attached_at: record.attached_at,
                monotonic: observed,
            });
        if deadline.absolute != absolute || deadline.attached_at != record.attached_at {
            // attach_in can change phase outside this service. Preserve the
            // earlier clock calibration so a stalled wall clock cannot grant
            // extra time before the first read observes the committed claim.
            let calibrated = (i128::from(deadline.monotonic) + i128::from(absolute)
                - i128::from(deadline.absolute))
            .clamp(0, i128::from(u64::MAX)) as u64;
            *deadline = Deadline {
                absolute,
                attached_at: record.attached_at,
                monotonic: calibrated.min(observed),
            };
        }
        deadline.monotonic = deadline.monotonic.min(observed);
        observation.effective_now >= absolute || observation.monotonic >= deadline.monotonic
    }

    pub fn attached(&mut self, record: &SecretRecord, observation: &Observation) {
        // This helper observes the actual commit. Replays retain the existing
        // monotonic limit instead of restarting the remaining UTC duration.
        let same_claim = self
            .deadlines
            .get(&record.context.id)
            .is_some_and(|deadline| {
                deadline.attached_at == record.attached_at
                    && Some(deadline.absolute) == record.expires_at
            });
        if !same_claim {
            self.deadlines.remove(&record.context.id);
        }
        self.expired(record, observation);
    }

    pub fn forget(&mut self, id: &str) {
        self.deadlines.remove(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::{GrantScope, SecretContext, StageContext};
    fn staged() -> SecretRecord {
        SecretRecord {
            context: SecretContext {
                id: format!("sec_{}", uuid::Uuid::new_v4()),
                device_id: "synthetic-device".into(),
                creator: "synthetic-user".into(),
                name: "SYNTHETIC".into(),
                ttl_seconds: Some(10),
                grant: GrantScope::Project {
                    project_id: "synthetic-project".into(),
                },
                stage: StageContext {
                    owner_id: "synthetic-project".into(),
                    conversation_id: "synthetic-conversation".into(),
                    generation: "synthetic-generation".into(),
                    draft_id: None,
                },
            },
            created_at: 1000,
            attached_at: None,
            expires_at: None,
            revoked_at: None,
            revocation_reason: None,
            revision: 1,
            message_operation_id: None,
            upload_request_id: uuid::Uuid::new_v4().to_string(),
            upload_digest: "a".repeat(64),
            blob_present: true,
        }
    }
    #[test]
    fn external_attachment_replaces_stage_deadline_even_if_first_read_is_late() {
        let mut state = ClockState::new(Arc::new(SystemClock::default()), 1000);
        let mut record = staged();
        assert!(!state.expired(
            &record,
            &Observation {
                now: 1000,
                effective_now: 1000,
                monotonic: 0,
                rollback: false
            }
        ));
        // Message admission commits through attach_in rather than the storage
        // helper. Its first read occurs after TTL while the wall clock stalls.
        record.attached_at = Some(1100);
        record.expires_at = Some(1110);
        assert!(state.expired(
            &record,
            &Observation {
                now: 1100,
                effective_now: 1100,
                monotonic: 110,
                rollback: false
            }
        ));
    }
    #[test]
    fn external_long_ttl_does_not_keep_the_shorter_stage_deadline() {
        let mut state = ClockState::new(Arc::new(SystemClock::default()), 1000);
        let mut record = staged();
        assert!(!state.expired(
            &record,
            &Observation {
                now: 1000,
                effective_now: 1000,
                monotonic: 0,
                rollback: false
            }
        ));
        record.context.ttl_seconds = Some(90_000);
        record.attached_at = Some(1100);
        record.expires_at = Some(91_100);
        assert!(!state.expired(
            &record,
            &Observation {
                now: 1100,
                effective_now: 1100,
                monotonic: 86_400,
                rollback: false
            }
        ));
        assert!(state.expired(
            &record,
            &Observation {
                now: 1100,
                effective_now: 1100,
                monotonic: 90_100,
                rollback: false
            }
        ));
    }
}
