//! Immutable admission and identity overlays for independent SQL reads.
//! Producers publish while already owning the app; readers never acquire it.

pub(in crate::app) mod mcp_tasks;

use super::conversations::sql_read::{ThreadReadContext, ThreadReadFence};
use super::tracker::{TaskReadContext, TaskReadFence};
use super::AppState;
use crate::api::{self, ApiError};
use crate::store::{ReadConnection, ReadTransaction};
use crate::transport::Frame;
use arc_swap::ArcSwapOption;
use serde_json::Value;
use std::sync::Arc;

pub(in crate::app) type ReadContextSlot = Arc<ArcSwapOption<ReadContext>>;

#[derive(Clone)]
pub(in crate::app) struct ReadContext {
    tasks: TaskReadContext,
    threads: ThreadReadContext,
    deleting_project: bool,
}

/// Captured once when the handler is constructed, rather than consulting
/// AppState for update admission or for any part of a read's result.
enum ReadFence {
    Thread(ThreadReadFence),
    Task(TaskReadFence),
}

pub(in crate::app) struct ReadService {
    context: ReadContextSlot,
    admission: Option<Arc<crate::update::AdmissionGate>>,
}

impl AppState {
    pub(in crate::app) fn initialize_read_context(&self) {
        if self.read_context.load().is_some() {
            return;
        }
        self.read_context.store(Some(Arc::new(ReadContext {
            tasks: self.task_read_context(),
            threads: self.thread_read_context(),
            deleting_project: self.project_deletion_in_progress,
        })));
    }

    /// Only this owner's durable save can add identities to read admission.
    /// Failed saves may remove live identities, but cannot install a candidate
    /// roster or change the Store-free fallback's committed fields.
    pub(in crate::app) fn publish_read_owner(&self, owner: &str, saved: bool) {
        #[cfg(test)]
        let _phase = crate::timing::phase_samples::phase("read_context.owner");
        let Some(previous) = self.read_context.load_full() else {
            return;
        };
        let mut next = (*previous).clone();
        if saved {
            next.threads.update_owner(self, owner);
        } else {
            next.threads.retain_owner_agents(self, owner);
        }
        next.tasks = self.task_read_context();
        next.tasks
            .retain_live_owners(&previous.tasks, self, Some((owner, saved)));
        next.deleting_project = self.project_deletion_in_progress;
        self.read_context.store(Some(Arc::new(next)));
    }

    pub(in crate::app) fn forget_read_owner(&self, owner: &str) {
        self.publish_owner_snapshot_projection(owner);
        let Some(previous) = self.read_context.load_full() else {
            return;
        };
        let mut next = (*previous).clone();
        next.threads.forget_owner(owner);
        next.tasks = self.task_read_context();
        next.tasks.retain_live_owners(&previous.tasks, self, None);
        next.tasks.forget_live_owner(owner);
        self.read_context.store(Some(Arc::new(next)));
    }

    /// Project/workspace links are published independently
    /// of roster admission. A board invalidation cannot expose a failed save.
    pub(in crate::app) fn publish_read_metadata(&self) {
        self.publish_snapshot_projection();
        self.publish_read_metadata_only();
    }

    /// Refresh admission metadata after a separately prepared publication.
    /// Attention changes do not establish that a roster save succeeded.
    pub(in crate::app) fn publish_read_metadata_only(&self) {
        #[cfg(test)]
        let _phase = crate::timing::phase_samples::phase("read_context.metadata");
        let Some(previous) = self.read_context.load_full() else {
            return;
        };
        let mut next = ReadContext {
            tasks: self.task_read_context(),
            threads: previous.threads.clone(),
            deleting_project: self.project_deletion_in_progress,
        };
        next.tasks.retain_live_owners(&previous.tasks, self, None);
        self.read_context.store(Some(Arc::new(next)));
    }

    /// Presence changes only the session carried by task reads. Keep the
    /// committed roster and path observations without preparing a new board.
    pub(in crate::app) fn publish_read_user_session(&self) {
        let Some(previous) = self.read_context.load_full() else {
            return;
        };
        let mut next = (*previous).clone();
        next.tasks.set_user_session(self.user_session);
        self.read_context.store(Some(Arc::new(next)));
    }

    /// A receipt transition must not republish a borrowed or unsaved roster.
    pub(in crate::app) fn publish_read_operations(&self) {
        let Some(previous) = self.read_context.load_full() else {
            return;
        };
        let mut next = (*previous).clone();
        next.threads.refresh_operations(self);
        self.read_context.store(Some(Arc::new(next)));
    }

    pub(in crate::app) fn read_service(&self) -> ReadService {
        ReadService {
            context: Arc::clone(&self.read_context),
            admission: self.update_admission(),
        }
    }
}

impl ReadService {
    pub(in crate::app) fn call(
        &self,
        frame: Frame,
        mut connection: Option<ReadConnection>,
    ) -> Value {
        let id = frame.payload.get("id").cloned().unwrap_or(Value::Null);
        let method = frame.payload["method"].as_str().unwrap_or("");
        let params = frame
            .payload
            .get("params")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        let result = self.read(method, &params, connection.as_mut());
        api::reply(id, result)
    }

    fn read(
        &self,
        method: &str,
        params: &Value,
        connection: Option<&mut ReadConnection>,
    ) -> Result<Value, ApiError> {
        let _lease = self
            .admission
            .as_ref()
            .map(|gate| {
                gate.try_enter()
                    .ok_or_else(|| ApiError::busy("bridge update handoff in progress"))
            })
            .transpose()?;
        let params = crate::api::v1::normalize_read_params(method, params)?;
        let context = self
            .context
            .load_full()
            .expect("read context initialized before admission");
        if context.deleting_project && method.starts_with("tasks.") {
            return Err(ApiError::unavailable(
                super::rpc::PROJECT_DELETION_IN_PROGRESS,
            ));
        }
        let (result, fence) = match connection {
            Some(connection) => connection
                .read(|read| Self::query(&context, Some(read), method, &params))
                .map_err(|error| ApiError::classify(error.to_string()))??,
            None => Self::query(&context, None, method, &params)?,
        };
        self.validate_fence(&fence)?;
        crate::api::v1::check_read_result(method, result)
    }

    fn query(
        context: &ReadContext,
        transaction: Option<&ReadTransaction<'_>>,
        method: &str,
        params: &Value,
    ) -> Result<(Value, ReadFence), ApiError> {
        if method.starts_with("thread.") {
            let (answer, fence) = context
                .threads
                .read_scoped(transaction, method, params)
                .map_err(thread_error)?;
            return Ok((answer, ReadFence::Thread(fence)));
        }
        let read = transaction
            .ok_or_else(|| task_error("tasks need a durable store; this bridge has none".into()))?;
        let fence = context
            .tasks
            .validate_scope(read, method, params)
            .map_err(task_error)?;
        let answer = match method {
            "tasks.list" => context.tasks.tasks_list(read, params),
            "tasks.get" => context.tasks.tasks_get(read, params),
            "tasks.waiting_on.blocks" => context.tasks.tasks_waiting_on_blocks(read, params),
            _ => return Err(ApiError::unknown_method(method)),
        }
        .map_err(task_error)?;
        Ok((answer, ReadFence::Task(fence)))
    }
    fn validate_fence(&self, fence: &ReadFence) -> Result<(), ApiError> {
        let latest = self.context.load_full().expect("initialized context");
        match fence {
            ReadFence::Thread(fence) => latest.threads.validate_fence(fence).map_err(thread_error),
            ReadFence::Task(_) if latest.deleting_project => Err(ApiError::unavailable(
                super::rpc::PROJECT_DELETION_IN_PROGRESS,
            )),
            ReadFence::Task(fence) => latest.tasks.validate_fence(fence).map_err(task_error),
        }
    }
}

fn thread_error(error: String) -> ApiError {
    crate::api::v1::thread::refine(ApiError::classify(error))
}

fn task_error(error: String) -> ApiError {
    crate::api::v1::tasks::refine(ApiError::classify(error))
}
