use super::*;
use crate::secrets::SecretCaller;
use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct UnauthenticatedFailures {
    pending: i64,
    last_flush: Option<Instant>,
}

impl SecretService {
    /// Only a private authenticated local endpoint may call this method.
    /// `is_current` must read the latest issued-session authority without the
    /// global app lock; it is evaluated at the metadata linearization point.
    pub fn release(
        &self,
        caller: &SecretCaller,
        id: &str,
        request_id: &str,
        is_current: impl FnOnce() -> bool,
    ) -> Result<SecretValue> {
        let _job = self.job()?;
        let mut state = self.state();
        let observed_at = state.wall_now();
        // Load and decrypt off the SQLite writer. The final transaction compares
        // this immutable context/revision again before it commits the audit.
        let candidate = self.release_candidate(caller, id);
        self.inner.writer.secrets_release(
            caller,
            id,
            request_id,
            observed_at,
            is_current,
            |tx, record| {
                let observation = state.observe_in(tx)?;
                self.release_status(record, &mut state, &observation, tx)?;
                let (before, value) = candidate?.ok_or(SecretError::Unavailable)?;
                if before.context != record.context || before.revision != record.revision {
                    return Err(SecretError::Unavailable);
                }
                value
            },
        )
    }

    fn release_candidate(
        &self,
        caller: &SecretCaller,
        id: &str,
    ) -> Result<Option<(SecretRecord, Result<SecretValue>)>> {
        if validate_id(id).is_err() {
            return Ok(None);
        }
        let Some(record) = self.inner.writer.secret_record(id)? else {
            return Ok(None);
        };
        if !crate::store::secrets::scope_allows(caller, &record)
            || record.revoked_at.is_some()
            || record.attached_at.is_none()
            || !record.blob_present
        {
            return Ok(None);
        }
        let value = self
            .vault()
            .and_then(|vault| vault.read(&record.context))
            .map_err(|_| SecretError::Unavailable);
        Ok(Some((record, value)))
    }

    fn release_status(
        &self,
        record: &SecretRecord,
        state: &mut ClockState,
        observation: &Observation,
        tx: &rusqlite::Transaction<'_>,
    ) -> Result<()> {
        if record.revoked_at.is_some() {
            return match record.revocation_reason.as_deref() {
                Some("expired") => Err(SecretError::Expired),
                Some("revoked") => Err(SecretError::Revoked),
                _ => Err(SecretError::Unavailable),
            };
        }
        if record.attached_at.is_none() {
            return Err(SecretError::Unavailable);
        }
        if state.expired(record, observation) {
            crate::store::secrets::expire_in(tx, &record.context.id, observation.effective_now)?;
            return Err(SecretError::Expired);
        }
        if observation.rollback && record.context.ttl_seconds.is_some() {
            return Err(SecretError::Unavailable);
        }
        if !record.blob_present {
            return Err(SecretError::Unavailable);
        }
        Ok(())
    }

    /// Coalesce failures without recording a claimed ID, capability, selector,
    /// payload or argv. At most one durable counter update per minute; a sweep
    /// flushes a pending batch. A failed write retains the bounded counter.
    pub fn record_unauthenticated_failure(&self) {
        let now = self.state().wall_now();
        {
            let mut failures = self
                .inner
                .unauthenticated
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            failures.pending = failures.pending.saturating_add(1);
        }
        self.flush_unauthenticated_failures(now, false);
    }

    pub(super) fn flush_unauthenticated_failures(&self, now: i64, force: bool) {
        let mut failures = self
            .inner
            .unauthenticated
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let due = force
            || failures
                .last_flush
                .is_none_or(|last| last.elapsed() >= Duration::from_secs(60));
        if failures.pending == 0 || !due || now < 0 {
            return;
        }
        // Failed persistence must be coalesced too, rather than turning an
        // unavailable database into one blocking write per rejected request.
        failures.last_flush = Some(Instant::now());
        if self
            .inner
            .writer
            .secrets_audit_unauthenticated(failures.pending, now)
            .is_ok()
        {
            failures.pending = 0;
        }
    }
}
