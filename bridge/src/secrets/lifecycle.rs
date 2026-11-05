use super::clock::{ClockState, Observation, SystemClock};
use super::vault::Vault;
use super::{
    validate_id, Clock, Result, SecretContext, SecretError, SecretRecord, SecretValue,
    MAX_CONCURRENT_JOBS,
};
use crate::store::StoreWriter;
use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

type ChangeObserver = Arc<dyn Fn(&SecretRecord) + Send + Sync>;

struct Inner {
    device_id: String,
    writer: StoreWriter,
    vault: Option<Vault>,
    closed: AtomicBool,
    state: Mutex<ClockState>,
    jobs: Arc<tokio::sync::Semaphore>,
    unauthenticated: Mutex<release::UnauthenticatedFailures>,
    change_observer: Mutex<Option<ChangeObserver>>,
}

/// Device-local storage service. Call from blocking work outside the app lock.
/// Externally usable reads must use the scoped, audited `release` method.
#[derive(Clone)]
pub struct SecretService {
    inner: Arc<Inner>,
}

#[derive(Default, Debug)]
pub struct SweepReport {
    pub deleted: usize,
    pub failed: usize,
}

impl SecretService {
    pub fn device_id(&self) -> &str {
        &self.inner.device_id
    }

    pub fn set_change_observer(&self, observer: ChangeObserver) {
        *self
            .inner
            .change_observer
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(observer);
    }

    pub(super) fn changed(&self, record: &SecretRecord) {
        let observer = self
            .inner
            .change_observer
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        if let Some(observer) = observer {
            if let Ok(record) = self.inner.writer.secret_metadata_resolve(record) {
                observer(&record);
            }
        }
    }

    pub fn open(state_root: &Path, device_id: &str, writer: StoreWriter) -> Result<Self> {
        Self::open_with_clock(
            state_root,
            device_id,
            writer,
            Arc::new(SystemClock::default()),
        )
    }

    pub fn open_with_clock(
        state_root: &Path,
        device_id: &str,
        writer: StoreWriter,
        clock: Arc<dyn Clock>,
    ) -> Result<Self> {
        Self::open_with_clock_and_opener(state_root, device_id, writer, clock, Vault::open)
    }

    pub(crate) fn open_with_clock_and_opener<F>(
        state_root: &Path,
        device_id: &str,
        writer: StoreWriter,
        clock: Arc<dyn Clock>,
        opener: F,
    ) -> Result<Self>
    where
        F: FnOnce(&Path, bool) -> Result<Vault>,
    {
        if device_id.is_empty() || device_id.len() > 128 {
            return Err(SecretError::Invalid);
        }
        let highwater = writer.secret_clock_highwater()?;
        let recovery_time = clock.wall_seconds().max(highwater).max(0);
        let vault = super::recovery::open_vault_with(state_root, &writer, recovery_time, opener)?;
        let service = Self {
            inner: Arc::new(Inner {
                device_id: device_id.into(),
                writer,
                vault,
                closed: AtomicBool::new(false),
                state: Mutex::new(ClockState::new(clock, highwater)),
                jobs: Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_JOBS)),
                unauthenticated: Mutex::new(release::UnauthenticatedFailures::default()),
                change_observer: Mutex::new(None),
            }),
        };
        if service.sweep().is_err() {
            service.inner.closed.store(true, Ordering::Release);
            super::recovery::report_unavailable(
                service.inner.writer.secrets_unretired_count().unwrap_or(0),
            );
        }
        Ok(service)
    }

    fn vault(&self) -> Result<&Vault> {
        if self.inner.closed.load(Ordering::Acquire) {
            return Err(SecretError::Unavailable);
        }
        self.inner.vault.as_ref().ok_or(SecretError::Unavailable)
    }

    fn state(&self) -> MutexGuard<'_, ClockState> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    fn job(&self) -> Result<tokio::sync::OwnedSemaphorePermit> {
        self.inner
            .jobs
            .clone()
            .try_acquire_owned()
            .map_err(|_| SecretError::Limit)
    }

    pub fn stage(
        &self,
        context: SecretContext,
        value: SecretValue,
        upload_request_id: String,
        upload_digest: String,
    ) -> Result<SecretRecord> {
        let job = self.job()?;
        self.stage_with_job(context, value, upload_request_id, upload_digest, job, None)
    }

    fn stage_with_job(
        &self,
        context: SecretContext,
        value: SecretValue,
        upload_request_id: String,
        upload_digest: String,
        _job: tokio::sync::OwnedSemaphorePermit,
        authority: Option<&crate::api::v1::secrets::SecretScope>,
    ) -> Result<SecretRecord> {
        let vault = self.vault()?;
        context.validate()?;
        if context.device_id != self.inner.device_id {
            return Err(SecretError::Unavailable);
        }
        let mut state = self.state();
        let observation = state.observe(&self.inner.writer)?;
        if observation.rollback {
            return Err(SecretError::ClockRollback);
        }
        // Never overwrite an already accepted blob while reconciling a retry.
        if self.inner.writer.secret_record(&context.id)?.is_some() {
            return Err(SecretError::Conflict);
        }
        let record = SecretRecord {
            context,
            created_at: observation.now,
            attached_at: None,
            expires_at: None,
            revoked_at: None,
            revocation_reason: None,
            revision: 1,
            message_operation_id: None,
            upload_request_id,
            upload_digest,
            blob_present: true,
        };
        self.inner.writer.secrets_stage_preflight(&record)?;
        vault.write(&record.context, &value)?;
        // A failed SQL commit deliberately leaves an inaccessible orphan. Only
        // the serialized sweeper removes it; no accepted value can be unlinked.
        match authority {
            Some(scope) => self.inner.writer.secrets_stage_authorized(&record, scope)?,
            None => self.inner.writer.secrets_stage(&record)?,
        }
        state.expired(&record, &observation);
        self.changed(&record);
        Ok(record)
    }

    /// Storage composition helper, not message admission. A secret-bearing send
    /// MUST use store::secrets::attach_in in the message's own transaction.
    pub fn attach(&self, ids: &[String], operation_id: &str) -> Result<Vec<SecretRecord>> {
        self.vault()?;
        let mut state = self.state();
        let observation = state.observe(&self.inner.writer)?;
        let before: Vec<_> = ids
            .iter()
            .map(|id| self.inner.writer.secret_record(id))
            .collect::<Result<_>>()?;
        for record in before
            .iter()
            .flatten()
            .filter(|record| record.attached_at.is_none())
        {
            if record.context.device_id != self.inner.device_id {
                return Err(SecretError::Unavailable);
            }
            self.require_unexpired(record, &mut state, &observation)?;
        }
        if observation.rollback {
            return Err(SecretError::ClockRollback);
        }
        let records = self
            .inner
            .writer
            .secrets_attach(ids, operation_id, observation.now)?;
        for (record, previous) in records.iter().zip(before) {
            if previous.is_some_and(|prior| prior.attached_at.is_none()) {
                state.attached(record, &observation);
            }
        }
        Ok(records)
    }

    pub fn read(&self, id: &str) -> Result<SecretValue> {
        let vault = self.vault()?;
        let _job = self.job()?;
        validate_id(id)?;
        let mut state = self.state();
        let observation = state.observe(&self.inner.writer)?;
        let record = self
            .inner
            .writer
            .secret_record(id)?
            .ok_or(SecretError::Unavailable)?;
        if record.context.device_id != self.inner.device_id
            || record.revoked_at.is_some()
            || record.attached_at.is_none()
            || !record.blob_present
        {
            return Err(SecretError::Unavailable);
        }
        self.require_unexpired(&record, &mut state, &observation)?;
        if observation.rollback && record.context.ttl_seconds.is_some() {
            return Err(SecretError::ClockRollback);
        }
        vault
            .read(&record.context)
            .map_err(|_| SecretError::Unavailable)
    }

    fn require_unexpired(
        &self,
        record: &SecretRecord,
        state: &mut ClockState,
        observation: &Observation,
    ) -> Result<()> {
        if state.expired(record, observation) {
            // Persist a monotonic expiry too: a restart while UTC is stalled
            // must not reopen a value that this process already denied.
            self.inner
                .writer
                .secrets_expire(&record.context.id, observation.effective_now)?;
            return Err(SecretError::Unavailable);
        }
        Ok(())
    }

    pub fn revoke(&self, id: &str) -> Result<SecretRecord> {
        validate_id(id)?;
        let mut state = self.state();
        let observation = state.observe(&self.inner.writer)?;
        let record = self
            .inner
            .writer
            .secrets_revoke(id, observation.effective_now)?;
        // Unlink failure does not undo or fail the committed revocation.
        self.delete_record(&record, &mut state);
        self.changed(&record);
        Ok(record)
    }

    fn delete_record(&self, record: &SecretRecord, state: &mut ClockState) -> bool {
        let Ok(vault) = self.vault() else {
            return false;
        };
        if vault.delete(&record.context.id).is_err() {
            return false;
        }
        if self
            .inner
            .writer
            .secrets_mark_deleted(&record.context.id)
            .is_err()
        {
            return false;
        }
        state.forget(&record.context.id);
        true
    }

    pub fn sweep(&self) -> Result<SweepReport> {
        let mut state = self.state();
        let observation = state.observe(&self.inner.writer)?;
        self.flush_unauthenticated_failures(observation.effective_now, true);
        self.inner
            .writer
            .secrets_audit_maintain(observation.effective_now)?;
        let Ok(vault) = self.vault() else {
            return Ok(SweepReport::default());
        };
        let records = self.inner.writer.secret_records()?;
        let mut retained = HashSet::new();
        let mut report = SweepReport::default();
        for record in &records {
            retained.insert(record.context.id.as_str());
            if self.retire_if_needed(record, &mut state, &observation)? {
                if let Some(current) = self.inner.writer.secret_record(&record.context.id)? {
                    self.changed(&current);
                }
                if self.delete_record(record, &mut state) {
                    report.deleted += 1;
                } else {
                    report.failed += 1;
                }
            }
        }
        for entry in vault.entries()? {
            if !retained.contains(entry.as_str()) {
                if vault.delete_entry(&entry).is_ok() {
                    report.deleted += 1;
                } else {
                    report.failed += 1;
                }
            }
        }
        Ok(report)
    }

    fn retire_if_needed(
        &self,
        record: &SecretRecord,
        state: &mut ClockState,
        observation: &Observation,
    ) -> Result<bool> {
        if record.revoked_at.is_some() {
            return Ok(true);
        }
        if record.context.device_id != self.inner.device_id {
            self.inner
                .writer
                .secrets_revoke(&record.context.id, observation.effective_now)?;
            return Ok(true);
        }
        if state.expired(record, observation) {
            self.inner
                .writer
                .secrets_expire(&record.context.id, observation.effective_now)?;
            return Ok(true);
        }
        Ok(false)
    }
}

#[cfg(test)]
#[path = "lifecycle_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "startup_sweep_tests.rs"]
mod startup_sweep_tests;

#[cfg(test)]
#[path = "release_tests.rs"]
mod release_tests;

#[path = "release.rs"]
mod release;

#[path = "staging.rs"]
mod staging;
