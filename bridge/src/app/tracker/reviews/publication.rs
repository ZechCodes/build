use super::super::StoredAnswer;
use crate::api::v1::reviews::{errors, ReviewPushParams, ReviewUpdateParams};
use crate::app::git::deferred::DeferredGitWork;
use crate::app::{AppState, DeferredGit, DeferredWork};
use crate::reviews::model::ReviewBranchBinding;
use crate::reviews::publication::explicit::{
    self, BaseRequest, BaseSelection, PushRequest, PushSource,
};
use crate::reviews::records::Review;
use crate::store::Store;
use crate::tracker::Actor;
use crate::workspace::{Workspace, WorkspaceStatus};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Mutex, Weak};

enum PublicationRequest {
    Push(PushRequest),
    Update(BaseRequest),
}

struct PublicationJob {
    store: Store,
    project_id: String,
    task_id: String,
    expected_version: u64,
    workspace: Workspace,
    bindings: Vec<ReviewBranchBinding>,
    shared: Option<Weak<Mutex<AppState>>>,
    registry_root: PathBuf,
    request: PublicationRequest,
}

impl DeferredGitWork for PublicationJob {
    fn run(&self, _: &Value) -> Result<Value, String> {
        self.run_publication().map_err(|message| {
            let mut details = json!({
                "task_id": self.task_id, "expected_version": self.expected_version,
                "recovery": "Refresh the review before retrying publication.",
            });
            if let Ok(Some(review)) = self.store.load_review_sync_state(&self.task_id) {
                details["current_version"] = json!(review.version);
            }
            errors::service(message, details)
        })
    }

    fn invalidate(&self, app: &mut AppState) {
        super::lifecycle::invalidates(app, &self.project_id, &self.task_id, &self.workspace.id);
    }

    fn invalidates_on_error(&self) -> bool {
        true
    }
}

impl PublicationJob {
    fn run_publication(&self) -> Result<Value, String> {
        let check = || self.check_workspace();
        match &self.request {
            PublicationRequest::Push(request) => {
                let result = explicit::push_checked(&self.store, request, &check)?;
                let mut value = super::projection::result(&self.store, result.review)?;
                value["sources"] =
                    serde_json::to_value(result.sources).map_err(|error| error.to_string())?;
                if let Some(recovery) = result.recovery {
                    value["recovery"] = json!(recovery);
                }
                Ok(value)
            }
            PublicationRequest::Update(request) => {
                let review = explicit::update_bases_checked(&self.store, request, &check)?;
                super::projection::result(&self.store, review)
            }
        }
    }

    fn check_workspace(&self) -> Result<(), String> {
        explicit::check_workspace(&self.workspace, &self.bindings)?;
        if let Some(shared) = &self.shared {
            let app = shared
                .upgrade()
                .ok_or("unavailable: bridge state disappeared")?;
            return super::opening::validate_live_workspace(&app.lock().unwrap(), &self.workspace);
        }
        super::opening::validate_recorded_workspace(&self.registry_root, &self.workspace)
    }
}

impl AppState {
    pub(crate) fn review_push(
        &mut self,
        params: ReviewPushParams,
        actor: Actor,
    ) -> Result<Value, String> {
        let refusal = |message| {
            errors::service(
                message,
                json!({"task_id":params.task_id,"expected_version":params.expected_version}),
            )
        };
        params.validate().map_err(refusal)?;
        let (project_id, review, workspace) = self
            .review_publication_context(&params.task_id)
            .map_err(refusal)?;
        super::lifecycle::version(&review, params.expected_version)?;
        let request = PushRequest {
            task_id: params.task_id.clone(),
            expected_version: params.expected_version,
            actor,
            sources: params
                .sources
                .into_iter()
                .map(|source| PushSource {
                    directory_id: source.directory_id,
                    expected_head: source.expected_head,
                    expected_received_head: source.expected_received_head,
                    force_with_lease: source.force_with_lease,
                })
                .collect(),
        };
        self.defer_publication(PublicationJob {
            store: self.tracker_store()?.clone(),
            project_id,
            task_id: params.task_id,
            expected_version: params.expected_version,
            workspace,
            bindings: review.bindings,
            shared: self.self_handle.clone(),
            registry_root: self.workspaces.root().into(),
            request: PublicationRequest::Push(request),
        });
        Ok(Value::Null)
    }

    pub(crate) fn review_update(
        &mut self,
        params: ReviewUpdateParams,
        actor: Actor,
    ) -> Result<Value, String> {
        let refusal = |message| {
            errors::service(
                message,
                json!({"task_id":params.task_id,"expected_version":params.expected_version}),
            )
        };
        params.validate().map_err(refusal)?;
        let (project_id, review, workspace) = self
            .review_publication_context(&params.task_id)
            .map_err(refusal)?;
        super::lifecycle::version(&review, params.expected_version)?;
        let request = BaseRequest {
            task_id: params.task_id.clone(),
            expected_version: params.expected_version,
            actor,
            bases: params
                .bases
                .into_iter()
                .map(|base| BaseSelection {
                    directory_id: base.directory_id,
                    branch: base.branch,
                })
                .collect(),
        };
        self.defer_publication(PublicationJob {
            store: self.tracker_store()?.clone(),
            project_id,
            task_id: params.task_id,
            expected_version: params.expected_version,
            workspace,
            bindings: review.bindings,
            shared: self.self_handle.clone(),
            registry_root: self.workspaces.root().into(),
            request: PublicationRequest::Update(request),
        });
        Ok(Value::Null)
    }

    fn defer_publication(&mut self, job: PublicationJob) {
        self.deferred_work = Some(DeferredWork::Git(Box::new(DeferredGit {
            call: Box::new(job),
            params: Value::Null,
            invalidates: true,
            #[cfg(test)]
            gate: None,
        })));
    }

    fn review_publication_context(
        &mut self,
        task_id: &str,
    ) -> Result<(String, Review, Workspace), String> {
        let (project_id, _) = self.tracker_task(task_id)?;
        let review = self
            .tracker_store()?
            .load_review(task_id)
            .stored()?
            .ok_or_else(|| format!("unknown review for task_id: {task_id}"))?;
        let workspace = self
            .workspaces
            .get(&review.workspace_id)
            .filter(|workspace| {
                workspace.project_id == project_id
                    && workspace.managed
                    && workspace.status == WorkspaceStatus::Ready
                    && workspace.archived_at.is_none()
            })
            .ok_or_else(|| "unavailable: original review workspace is unavailable".to_owned())?
            .clone();
        self.refuse_writers_while_reserved(&workspace.root)?;
        let sources = self.sources_for(&project_id)?;
        for binding in &review.bindings {
            let directory = workspace
                .directories
                .iter()
                .find(|directory| {
                    directory.id == binding.directory_id
                        && directory.source_id == binding.source_id
                        && directory.path == binding.working_repository
                })
                .ok_or("conflict: bound review workspace directory changed")?;
            if !sources.iter().any(|source| {
                source.id == binding.source_id && source.path == binding.source_repository
            }) {
                return Err("unavailable: bound review source is no longer configured".into());
            }
            self.refuse_writers_while_reserved(&directory.path)?;
            self.refuse_writers_while_reserved(&binding.source_repository)?;
        }
        Ok((project_id, review, workspace))
    }
}
