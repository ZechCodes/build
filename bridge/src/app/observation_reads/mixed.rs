//! Mixed readers split transactional metadata from their fully owned external work.
mod capture;
mod documents;
mod homes;
mod reviews;

use crate::api::ApiError;
use crate::app::board::archive::ArchiveReadInput;
use crate::app::conversations::sql_read::{AttachmentReadContext, AttachmentReadFence};
use crate::app::tracker::{TaskReadContext, TaskReadFence};
use crate::app::{require_str, AppState};
use crate::store::ReadTransaction;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone)]
pub(in crate::app) struct MixedReadContext {
    tasks: TaskReadContext,
    threads: AttachmentReadContext,
    homes: homes::Homes,
    attachment_root: PathBuf,
    documents: documents::Documents,
    archive: Arc<ArchiveReadInput>,
    sources: reviews::ReviewSources,
    #[cfg(test)]
    gate: Option<crate::app::OffLockGate>,
}

pub(super) use capture::MixedPatch;

pub(in crate::app) struct MixedJob {
    fence: MixedFence,
    work: Box<dyn FnOnce() -> Result<Value, ApiError> + Send>,
    #[cfg(test)]
    gate: Option<crate::app::OffLockGate>,
}

#[derive(Clone)]
pub(in crate::app) enum MixedFence {
    Archive,
    Document(String),
    Task(TaskReadFence),
    Thread(AttachmentReadFence),
}

impl MixedReadContext {
    pub fn capture(app: &AppState) -> Self {
        Self {
            tasks: app.task_scope_read_context(),
            threads: AttachmentReadContext::capture(app),
            homes: homes::Homes::capture(app),
            attachment_root: app.local_attachments_dir(),
            documents: documents::Documents::capture(app),
            archive: Arc::new(app.archive_read_input()),
            sources: reviews::ReviewSources::capture(app),
            #[cfg(test)]
            gate: app.off_lock_gate.clone(),
        }
    }

    pub fn prepare(
        &self,
        method: &str,
        params: &Value,
        transaction: Option<&ReadTransaction<'_>>,
    ) -> Result<MixedJob, ApiError> {
        match method {
            "archived.list" => {
                let archive = self.archive.clone();
                Ok(self.job(MixedFence::Archive, move || Ok(archive.read())))
            }
            "task.doc" | "task.stage_doc" => {
                let document = self
                    .documents
                    .prepare(method, params)
                    .map_err(ApiError::classify)?;
                let owner = require_str(params, "plan_id").map_err(ApiError::classify)?;
                Ok(self.job(MixedFence::Document(owner), move || {
                    document.run().map_err(ApiError::classify)
                }))
            }
            "thread.attachment" => self.thread_attachment(params),
            "tasks.attachment" | "tasks.review.get" | "tasks.review.diff" => {
                self.task_read(transaction, method, params)
            }
            _ => Err(ApiError::unknown_method(method)),
        }
    }

    fn job(
        &self,
        fence: MixedFence,
        work: impl FnOnce() -> Result<Value, ApiError> + Send + 'static,
    ) -> MixedJob {
        MixedJob {
            fence,
            work: Box::new(work),
            #[cfg(test)]
            gate: self.gate.clone(),
        }
    }

    fn thread_attachment(&self, params: &Value) -> Result<MixedJob, ApiError> {
        let owner = require_str(params, "entity_id").map_err(thread_error)?;
        let fence = self.threads.prepare(&owner, params).map_err(thread_error)?;
        require_str(params, "path").map_err(thread_error)?;
        let homes = self
            .homes
            .get(&owner, &self.attachment_root)
            .ok_or_else(|| thread_error("unknown conversation owner".into()))?;
        let params = params.clone();
        Ok(self.job(MixedFence::Thread(fence), move || {
            crate::app::conversations::read_thread_attachment(homes, &params).map_err(thread_error)
        }))
    }

    fn task_read(
        &self,
        transaction: Option<&ReadTransaction<'_>>,
        method: &str,
        params: &Value,
    ) -> Result<MixedJob, ApiError> {
        if method == "tasks.attachment" {
            require_str(params, "path").map_err(task_error)?;
        }
        let transaction = transaction
            .ok_or_else(|| task_error("tasks need a durable store; this bridge has none".into()))?;
        let fence = self
            .tasks
            .validate_scope(transaction, method, params)
            .map_err(task_error)?;
        if method == "tasks.attachment" {
            let home = self.attachment_root.clone();
            let params = params.clone();
            return Ok(self.job(MixedFence::Task(fence), move || {
                crate::app::tracker::read_task_attachment(&home, &params).map_err(task_error)
            }));
        }
        self.review(transaction, method, params, fence)
    }

    pub(in crate::app) fn prepare_mcp_review(
        &self,
        project: &str,
        method: &str,
        params: &Value,
        transaction: &ReadTransaction<'_>,
    ) -> Result<MixedJob, ApiError> {
        let id = require_str(params, "task_id").map_err(ApiError::classify)?;
        let fence = self
            .tasks
            .validate_scope(transaction, method, params)
            .map_err(task_error)?;
        if fence.project_id() != project {
            return Err(task_error(format!("unknown task_id: {id}")));
        }
        // The caller project is checked before loading review/snapshot/directory
        // content; the selected job then owns everything it will use off SQL.
        self.review(transaction, method, params, fence)
    }

    fn review(
        &self,
        transaction: &ReadTransaction<'_>,
        method: &str,
        params: &Value,
        fence: TaskReadFence,
    ) -> Result<MixedJob, ApiError> {
        let id = require_str(params, "task_id").map_err(ApiError::classify)?;
        let review = transaction
            .load_review(&id)
            .map_err(|error| ApiError::classify(error.to_string()))?;
        if method == "tasks.review.get" {
            let Some(review) = review else {
                return Ok(self.job(MixedFence::Task(fence), || Ok(json!({"review":null}))));
            };
            let (sync, intents) = if review.mode == crate::reviews::records::ReviewMode::PullRequest
            {
                (
                    transaction
                        .load_review_sync_observations(&id)
                        .map_err(|error| ApiError::classify(error.to_string()))?,
                    transaction
                        .load_review_merge_intents(&id)
                        .map_err(|error| ApiError::classify(error.to_string()))?,
                )
            } else {
                (Vec::new(), Vec::new())
            };
            let project = fence.project_id().to_string();
            let sources = self.sources.project(&project);
            return Ok(self.job(MixedFence::Task(fence), move || {
                Ok(reviews::result(
                    sources.destinations(&project, review),
                    sync,
                    intents,
                ))
            }));
        }
        let review = review
            .ok_or_else(|| ApiError::classify(format!("unknown review for task_id: {id}")))?;
        let params: crate::api::v1::reviews::ReviewDiffParams =
            serde_json::from_value(params.clone())
                .map_err(|error| ApiError::invalid_params(error.to_string()))?;
        let snapshot = review
            .snapshots
            .iter()
            .find(|snapshot| snapshot.id == params.snapshot_id)
            .ok_or_else(|| {
                ApiError::classify(format!("unknown snapshot_id: {}", params.snapshot_id))
            })?;
        let directory = snapshot
            .directories
            .iter()
            .find(|directory| directory.id == params.directory_id)
            .ok_or_else(|| {
                ApiError::classify(format!("unknown directory_id: {}", params.directory_id))
            })?
            .clone();
        Ok(self.job(MixedFence::Task(fence), move || {
            let result =
                crate::reviews::read::read(&directory, &params.read).map_err(ApiError::classify)?;
            serde_json::to_value(result).map_err(|error| ApiError::internal(error.to_string()))
        }))
    }

    pub fn validate(&self, fence: &MixedFence) -> Result<(), ApiError> {
        match fence {
            MixedFence::Archive => Ok(()),
            MixedFence::Document(id) if self.documents.contains(id) => Ok(()),
            MixedFence::Document(_) => Err(ApiError::classify("unknown plan_id".into())),
            MixedFence::Task(fence) => self.tasks.validate_fence(fence).map_err(task_error),
            MixedFence::Thread(fence) => self.threads.validate(fence).map_err(thread_error),
        }
    }
}

impl MixedJob {
    pub fn fence(&self) -> MixedFence {
        self.fence.clone()
    }
    pub fn run(self) -> Result<Value, ApiError> {
        #[cfg(test)]
        if let Some(gate) = self.gate {
            gate.arrive();
        }
        (self.work)()
    }
}

fn task_error(error: String) -> ApiError {
    crate::api::v1::tasks::refine(ApiError::classify(error))
}
fn thread_error(error: String) -> ApiError {
    crate::api::v1::thread::refine(ApiError::classify(error))
}
