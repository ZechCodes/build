//! Immutable admission and identity overlays for independent SQL reads.
//! Producers publish while already owning the app; readers never acquire it.

mod mcp_addresses;
mod mcp_bootstrap;
mod mcp_history;
#[cfg(test)]
pub(in crate::app) use mcp_addresses::metrics as mcp_metadata_metrics;
pub(in crate::app) use mcp_addresses::{FrozenMcpAddress, IssuedCodingAddress, McpAddresses};
pub(in crate::app) use mcp_bootstrap::{HarnessSettings, McpBootstrap};
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
    pub(in crate::app) mutations: super::mutations::MutationAuthority,
    addresses: Arc<McpAddresses>,
    bootstrap: Arc<McpBootstrap>,
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
            addresses: Arc::new(McpAddresses::capture(self)),
            bootstrap: Arc::new(McpBootstrap::capture(self)),
            deleting_project: self.project_deletion_in_progress,
            mutations: super::mutations::MutationAuthority::capture(self),
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
            Arc::make_mut(&mut next.addresses).update_owner(self, owner);
            if !self.snapshot_projection_initialized() {
                Arc::make_mut(&mut next.bootstrap).update_owner(self, owner);
            }
        } else {
            next.threads.retain_owner_agents(self, owner);
            Arc::make_mut(&mut next.addresses).retain_owner_agents(self, owner);
        }
        next.tasks = self.task_read_context();
        next.tasks
            .retain_live_owners(&previous.tasks, self, Some((owner, saved)));
        next.deleting_project = self.project_deletion_in_progress;
        next.mutations = super::mutations::MutationAuthority::capture(self);
        self.read_context.store(Some(Arc::new(next)));
    }

    /// Reset installs every alias and canonical head before this one store.
    pub(in crate::app) fn publish_read_owners(&self, owners: &std::collections::HashSet<String>) {
        let Some(previous) = self.read_context.load_full() else {
            return;
        };
        let mut next = (*previous).clone();
        for owner in owners {
            next.threads.update_owner(self, owner);
            Arc::make_mut(&mut next.addresses).update_owner(self, owner);
            if !self.snapshot_projection_initialized() {
                Arc::make_mut(&mut next.bootstrap).update_owner(self, owner);
            }
        }
        next.tasks = self.task_read_context();
        next.tasks
            .retain_saved_owners(&previous.tasks, self, owners);
        next.deleting_project = self.project_deletion_in_progress;
        next.mutations = super::mutations::MutationAuthority::capture(self);
        self.read_context.store(Some(Arc::new(next)));
    }

    pub(in crate::app) fn forget_read_owner(&self, owner: &str) {
        self.publish_owner_snapshot_projection(owner);
        let Some(previous) = self.read_context.load_full() else {
            return;
        };
        let mut next = (*previous).clone();
        next.threads.forget_owner(owner);
        Arc::make_mut(&mut next.addresses).forget_owner(self, owner);
        next.tasks = self.task_read_context();
        next.tasks.retain_live_owners(&previous.tasks, self, None);
        next.tasks.forget_live_owner(owner);
        next.mutations = super::mutations::MutationAuthority::capture(self);
        self.read_context.store(Some(Arc::new(next)));
    }

    /// Project/workspace links are published independently
    /// of roster admission. A board invalidation cannot expose a failed save.
    pub(in crate::app) fn publish_read_metadata(&self) {
        let _ownership = self.begin_snapshot_ownership();
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
        let mut addresses = Arc::clone(&previous.addresses);
        Arc::make_mut(&mut addresses).refresh_order(self);
        let mut next = ReadContext {
            tasks: self.task_read_context(),
            threads: previous.threads.clone(),
            addresses,
            bootstrap: Arc::clone(&previous.bootstrap),
            deleting_project: self.project_deletion_in_progress,
            mutations: super::mutations::MutationAuthority::capture(self),
        };
        next.tasks.retain_live_owners(&previous.tasks, self, None);
        self.read_context.store(Some(Arc::new(next)));
    }

    /// Before first snapshot warming, a listener reads its committed settings
    /// from this seed. A successful config save replaces only those fields.
    pub(in crate::app) fn publish_read_cold_harness_settings(&self) {
        if self.snapshot_projection_initialized() {
            return;
        }
        let Some(previous) = self.read_context.load_full() else {
            return;
        };
        let settings = &previous.bootstrap.settings;
        if settings.default_harness == self.default_harness
            && settings.role_models == self.role_models
        {
            return;
        }
        let mut next = (*previous).clone();
        Arc::make_mut(&mut next.bootstrap).settings = HarnessSettings {
            default_harness: self.default_harness,
            role_models: self.role_models.clone(),
        };
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
    pub(in crate::app) fn mcp_input_snapshot(
        &self,
    ) -> (Arc<McpAddresses>, Arc<McpBootstrap>, bool) {
        let current = self.context.load_full().expect("initialized read context");
        (
            Arc::clone(&current.addresses),
            Arc::clone(&current.bootstrap),
            current.deleting_project,
        )
    }

    #[cfg(test)]
    pub(in crate::app) fn mcp_addresses(&self) -> Arc<McpAddresses> {
        Arc::clone(
            &self
                .context
                .load_full()
                .expect("initialized read context")
                .addresses,
        )
    }

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
                gate.try_enter_read()
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
            "tasks.filterChoices" => context.tasks.tasks_filter_choices(read, params),
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
