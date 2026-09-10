use crate::app::AppState;
use crate::lifecycle::{PendingRow, Performed, WorktreeMutation};
use serde_json::Value;
use std::sync::Arc;

pub trait LifecycleSettlement<O>: Send + 'static {
    fn settle(self, state: &mut AppState, result: Result<O, String>) -> Result<Value, String>;
}

impl<O, F> LifecycleSettlement<O> for F
where
    F: FnOnce(&mut AppState, Result<O, String>) -> Result<Value, String> + Send + 'static,
{
    fn settle(self, state: &mut AppState, result: Result<O, String>) -> Result<Value, String> {
        self(state, result)
    }
}

pub struct AppReservation {
    pub row: Arc<PendingRow>,
}

impl AppReservation {
    pub fn row_only(row: Arc<PendingRow>) -> Self {
        Self { row }
    }
    fn roll_back(self, _state: &mut AppState) {}
}

struct PendingLifecycle<T, S> {
    reservation: AppReservation,
    task: T,
    settlement: S,
    #[cfg(test)]
    gate: Option<crate::test_support::off_lock::OffLockGate>,
}
struct CompletedLifecycle<O, S> {
    reservation: AppReservation,
    result: Result<Performed<O>, String>,
    settlement: S,
}

trait DeferredLifecycle: Send {
    fn run(self: Box<Self>) -> LifecycleOutcome;
}
trait ApplyLifecycle: Send {
    fn apply(self: Box<Self>, state: &mut AppState) -> Result<Value, String>;
}

pub struct WorktreeLifecycleJob(Box<dyn DeferredLifecycle>);
pub struct LifecycleOutcome(Box<dyn ApplyLifecycle>);

impl<T, S> DeferredLifecycle for PendingLifecycle<T, S>
where
    T: WorktreeMutation,
    S: LifecycleSettlement<T::Output>,
{
    fn run(self: Box<Self>) -> LifecycleOutcome {
        #[cfg(test)]
        if let Some(gate) = &self.gate {
            gate.arrive();
        }
        LifecycleOutcome(Box::new(CompletedLifecycle {
            reservation: self.reservation,
            result: self.task.perform(),
            settlement: self.settlement,
        }))
    }
}

impl<O, S> ApplyLifecycle for CompletedLifecycle<O, S>
where
    O: Send + 'static,
    S: LifecycleSettlement<O>,
{
    fn apply(self: Box<Self>, state: &mut AppState) -> Result<Value, String> {
        let CompletedLifecycle {
            reservation,
            result,
            settlement,
        } = *self;
        let project_id = reservation.row.project_id.clone();
        state.release_row(&reservation.row.entity_id);
        match result {
            Ok(Performed { change, output }) => {
                state.amend_checkouts(project_id.as_deref(), &change);
                match settlement.settle(state, Ok(output)) {
                    Ok(answer) => Ok(answer),
                    Err(error) => {
                        state.amend_checkouts(project_id.as_deref(), &change);
                        reservation.roll_back(state);
                        Err(error)
                    }
                }
            }
            Err(error) => {
                let settled = settlement.settle(state, Err(error));
                reservation.roll_back(state);
                settled
            }
        }
    }
}

impl WorktreeLifecycleJob {
    pub fn run(self) -> LifecycleOutcome {
        self.0.run()
    }
}
impl LifecycleOutcome {
    pub fn apply(self, state: &mut AppState) -> Result<Value, String> {
        self.0.apply(state)
    }
}

#[cfg(not(test))]
pub fn bind_lifecycle<T, S>(
    reservation: AppReservation,
    task: T,
    settlement: S,
) -> WorktreeLifecycleJob
where
    T: WorktreeMutation,
    S: LifecycleSettlement<T::Output>,
{
    WorktreeLifecycleJob(Box::new(PendingLifecycle {
        reservation,
        task,
        settlement,
        #[cfg(test)]
        gate: None,
    }))
}

#[cfg(test)]
pub fn bind_lifecycle_with_gate<T, S>(
    reservation: AppReservation,
    task: T,
    settlement: S,
    gate: Option<crate::test_support::off_lock::OffLockGate>,
) -> WorktreeLifecycleJob
where
    T: WorktreeMutation,
    S: LifecycleSettlement<T::Output>,
{
    WorktreeLifecycleJob(Box::new(PendingLifecycle {
        reservation,
        task,
        settlement,
        gate,
    }))
}
