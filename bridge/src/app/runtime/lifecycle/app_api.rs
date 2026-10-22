#[cfg(not(test))]
use super::runner::bind_lifecycle;
#[cfg(test)]
use super::runner::bind_lifecycle_with_gate;
use super::runner::{AppReservation, LifecycleOutcome, LifecycleSettlement, WorktreeLifecycleJob};
use crate::app::{AppState, DeferredWork};
use crate::lifecycle::{PendingRow, WorktreeMutation};
use serde_json::Value;

impl AppState {
    pub(crate) fn reserve_lifecycle<T, S>(
        &mut self,
        row: PendingRow,
        task: T,
        settlement: S,
    ) -> Result<WorktreeLifecycleJob, String>
    where
        T: WorktreeMutation,
        S: LifecycleSettlement<T::Output>,
    {
        let row = self.reserve_row(row)?;
        Ok(self.lifecycle_job(AppReservation::row_only(row), task, settlement))
    }
    pub(crate) fn defer_lifecycle<T, S>(
        &mut self,
        row: PendingRow,
        task: T,
        settlement: S,
    ) -> Result<Value, String>
    where
        T: WorktreeMutation,
        S: LifecycleSettlement<T::Output>,
    {
        let job = self.reserve_lifecycle(row, task, settlement)?;
        self.deferred_work = Some(DeferredWork::Lifecycle(Box::new(job)));
        Ok(Value::Null)
    }
    pub(crate) fn defer_lifecycle_holding<T, S>(
        &mut self,
        row: PendingRow,
        take: impl FnOnce(&mut AppState) -> (T, S),
    ) -> Result<Value, String>
    where
        T: WorktreeMutation,
        S: LifecycleSettlement<T::Output>,
    {
        let row = self.reserve_row(row)?;
        let (task, settlement) = take(self);
        let job = self.lifecycle_job(AppReservation::row_only(row), task, settlement);
        self.deferred_work = Some(DeferredWork::Lifecycle(Box::new(job)));
        Ok(Value::Null)
    }
    pub(crate) fn apply_lifecycle(&mut self, outcome: LifecycleOutcome) -> Result<Value, String> {
        outcome.apply(self)
    }
    fn lifecycle_job<T, S>(
        &self,
        mut reservation: AppReservation,
        task: T,
        settlement: S,
    ) -> WorktreeLifecycleJob
    where
        T: WorktreeMutation,
        S: LifecycleSettlement<T::Output>,
    {
        if let Some(checkout_id) = reservation.row.checkout_id.as_deref() {
            // A removal receipt may arrive after the same key hosts a new
            // session. Retain the registrations the job actually started for.
            reservation.terminal_cleanup = self
                .session_registry
                .terminal_cleanup_candidates()
                .into_iter()
                .filter(|candidate| {
                    crate::worktree::external_worktree_id(&candidate.key().root) == checkout_id
                })
                .collect();
        }
        #[cfg(test)]
        {
            bind_lifecycle_with_gate(reservation, task, settlement, self.off_lock_gate.clone())
        }
        #[cfg(not(test))]
        {
            bind_lifecycle(reservation, task, settlement)
        }
    }
}
