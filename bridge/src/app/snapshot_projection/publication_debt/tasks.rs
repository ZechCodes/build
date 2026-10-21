//! Required task publication is the captured global task cut plus its own note tail.
use super::*;
use crate::store::delivery_origins::{PublicationCandidate, TaskPublicationTransfer};
use crate::timing::write_wait::{Obligation, WaitGuard, WriteId};
use std::sync::atomic::{AtomicU64, Ordering};
mod scope;
pub(in crate::app) use scope::TaskPublicationWork;

pub(in crate::app) struct TaskPublicationTicket(u64);
#[derive(Default)]
pub(in crate::app::snapshot_projection) struct TaskPublications {
    inputs: AtomicU64,
    next: AtomicU64,
    state: Mutex<State>,
}
#[derive(Default)]
struct State {
    accepted: u64,
    pending: BTreeMap<u64, Pending>,
}
struct Pending {
    project: String,
    target: Option<u64>,
    metadata: bool,
    processing: usize,
    required: Vec<Required>,
}
struct Required {
    token: Obligation,
    cancelled: bool,
    queued: Vec<WaitGuard>,
    own_queue_origins: Vec<WriteId>,
    _attempt: Option<Arc<crate::store::delivery_origins::Attempt>>,
}
impl TaskPublications {
    /// Authoritative task inputs and this revision change under watched_tasks.
    pub(in crate::app) fn changed_inputs(&self) -> u64 {
        self.inputs.fetch_add(1, Ordering::Relaxed) + 1
    }
    /// Capture under the same watched_tasks mutex as every cloned row Arc.
    pub(in crate::app) fn input_revision(&self) -> u64 {
        self.inputs.load(Ordering::Relaxed)
    }
    fn begin(
        &self,
        project: &str,
        transfer: Option<TaskPublicationTransfer>,
        author: Option<WriteContext>,
    ) -> Option<TaskPublicationTicket> {
        let mut required = Vec::new();
        if let Some(transfer) = transfer {
            required.push(Required {
                token: transfer.original,
                cancelled: transfer.cancelled,
                queued: Vec::new(),
                own_queue_origins: Vec::new(),
                _attempt: Some(transfer._attempt),
            });
        }
        if let Some(author) = author {
            required.push(Required {
                token: author.child(),
                cancelled: false,
                queued: Vec::new(),
                own_queue_origins: Vec::new(),
                _attempt: None,
            });
        }
        if required.is_empty() {
            return None;
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        self.state.lock().unwrap().pending.insert(
            id,
            Pending {
                project: project.into(),
                target: None,
                metadata: false,
                processing: 0,
                required,
            },
        );
        Some(TaskPublicationTicket(id))
    }
    fn capture(&self, ticket: Option<&TaskPublicationTicket>, revision: u64) {
        if let Some(ticket) = ticket {
            self.state
                .lock()
                .unwrap()
                .pending
                .get_mut(&ticket.0)
                .unwrap()
                .target = Some(revision);
        }
    }
    pub(in crate::app) fn refreshed_project(&self, project: &str, revision: u64) {
        for pending in self.state.lock().unwrap().pending.values_mut() {
            if pending.project == project && pending.metadata && pending.target.is_none() {
                pending.target = Some(revision);
            }
        }
    }
    fn metadata(&self, ticket: Option<&TaskPublicationTicket>) {
        if let Some(ticket) = ticket {
            let completed = {
                let mut state = self.state.lock().unwrap();
                state.pending.get_mut(&ticket.0).unwrap().metadata = true;
                state.take_completed()
            };
            finish(completed);
        }
    }
    fn accept(&self, revision: u64) {
        let completed = {
            let mut state = self.state.lock().unwrap();
            state.accepted = state.accepted.max(revision);
            state.take_completed()
        };
        finish(completed);
    }
}
impl State {
    fn take_completed(&mut self) -> Vec<Pending> {
        let ready: Vec<_> = self
            .pending
            .iter()
            .filter(|(_, pending)| {
                pending.metadata && pending.target.is_some_and(|target| target <= self.accepted)
            })
            .map(|(id, _)| *id)
            .collect();
        ready
            .into_iter()
            .map(|id| self.pending.remove(&id).unwrap())
            .collect()
    }
}
fn finish(completed: Vec<Pending>) {
    for pending in completed {
        for required in pending.required {
            drop(required.queued);
            if required.cancelled {
                required.token.cancel();
            } else {
                required.token.complete();
            }
        }
    }
}
impl AppState {
    pub(in crate::app) fn begin_task_publication(
        &self,
        project: &str,
        retiring: Option<&PublicationCandidate>,
    ) -> Option<TaskPublicationTicket> {
        if !self.snapshot_projection_initialized() {
            return None;
        }
        self.snapshot_observations.task_publications.begin(
            project,
            retiring.and_then(PublicationCandidate::transfer),
            self.mutation_intent_context(),
        )
    }
    pub(in crate::app) fn capture_task_publication_inputs(
        &self,
        ticket: Option<&TaskPublicationTicket>,
    ) {
        let _rows =
            crate::timing::resource_lock!(self.snapshot_observations.watched_tasks).unwrap();
        let revision = self
            .snapshot_observations
            .task_publications
            .input_revision();
        self.snapshot_observations
            .task_publications
            .capture(ticket, revision);
    }
    pub(in crate::app) fn finish_task_publication_metadata(
        &self,
        ticket: Option<&TaskPublicationTicket>,
    ) {
        self.refresh_publication_barrier();
        self.snapshot_observations
            .task_publications
            .metadata(ticket);
    }
    pub(in crate::app) fn accept_task_publication(&self, revision: Option<u64>) {
        if let Some(revision) = revision {
            self.snapshot_observations
                .task_publications
                .accept(revision);
        }
    }
    pub(in crate::app) fn begin_task_publication_work(
        &self,
        ticket: Option<&TaskPublicationTicket>,
    ) -> TaskPublicationWork {
        // This frontier selects live work only; it never supplies acceptance
        // coverage. Proof comes from the Delta's coherent row-and-revision cut.
        let revision = self
            .snapshot_observations
            .task_publications
            .input_revision();
        self.begin_task_publication_work_for(ticket, None, Some(revision))
    }
    pub(in crate::app) fn begin_task_publication_coverage_work(
        &self,
        coverage: Option<u64>,
    ) -> TaskPublicationWork {
        self.begin_task_publication_work_for(None, None, coverage)
    }
    pub(in crate::app) fn begin_task_publication_refresh_work(
        &self,
        project: &str,
    ) -> TaskPublicationWork {
        self.begin_task_publication_work_for(None, Some(project), None)
    }
    fn begin_task_publication_work_for(
        &self,
        ticket: Option<&TaskPublicationTicket>,
        project: Option<&str>,
        coverage: Option<u64>,
    ) -> TaskPublicationWork {
        self.refresh_publication_barrier();
        self.snapshot_observations.task_publications.work(
            Arc::clone(&self.snapshot_observations.publication_debts),
            ticket,
            project,
            coverage,
        )
    }
}
