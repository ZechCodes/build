//! Resolve caller-owned selections under the app lock; publish off it.
mod delivery;
mod validation;

use crate::api::v1::reviews::{
    errors, ReviewOpenParams, ReviewOpenResult, ReviewPushInstruction, ReviewReviewer,
};
use crate::app::git::deferred::DeferredGitWork;
use crate::app::mcp::McpConversationGeneration;
use crate::app::{AppState, DeferredGit, DeferredWork};
use crate::reviews::model::{ReviewOpeningRequest, ReviewOpeningState};
use crate::reviews::opening::{OpenReviewRequest, OpenedReview};
use crate::tracker::{Actor, TaskEventKind, TimelineEntry};
use crate::workspace::WorkspaceStatus;
use delivery::SettlementHooks;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::{Mutex, Weak};
use validation::{selections, validate_actor};
pub(super) use validation::{validate_live_workspace, validate_recorded_workspace};

struct OpeningJob {
    store: crate::store::Store,
    project_id: String,
    request: OpenReviewRequest,
    hooks: LiveOpeningHooks,
    watch: bool,
}

struct LiveOpeningHooks {
    shared: Option<Weak<Mutex<AppState>>>,
    registry_root: PathBuf,
    project_id: String,
    project_path: String,
    request_id: String,
    store: crate::store::Store,
    /// The creator's admitted conversation, retained while Git runs off lock.
    sender: Option<McpConversationGeneration>,
}

impl AppState {
    pub(crate) fn review_open(
        &mut self,
        params: ReviewOpenParams,
        actor: Actor,
    ) -> Result<Value, String> {
        let workspace_id = params.workspace_id.clone();
        self.prepare_review_open(params, actor).map_err(|error| {
            let directories = self
                .workspaces
                .get(&workspace_id)
                .map(|workspace| &workspace.directories);
            let directory_id = directories
                .and_then(|directories| {
                    directories
                        .iter()
                        .find(|directory| error.contains(&directory.id))
                        .or_else(|| (directories.len() == 1).then(|| &directories[0]))
                })
                .map(|directory| directory.id.as_str());
            opening_error(error, &workspace_id, directory_id)
        })
    }

    fn prepare_review_open(
        &mut self,
        params: ReviewOpenParams,
        actor: Actor,
    ) -> Result<Value, String> {
        params.validate()?;
        let workspace = self
            .workspaces
            .get(&params.workspace_id)
            .ok_or_else(|| format!("unknown workspace_id: {}", params.workspace_id))?
            .clone();
        if !workspace.managed || workspace.status != WorkspaceStatus::Ready {
            return Err("invalid review params: opening requires a ready managed workspace".into());
        }
        validate_live_workspace(self, &workspace)?;
        let store = self.tracker_store()?.clone();
        let project_path = self.tracker_project_path(&workspace.project_id)?;
        let existing = store
            .load_review_opening(&project_path, &params.request_id)
            .map_err(|error| error.to_string())?;
        validate_actor(
            self,
            &workspace.project_id,
            &actor,
            existing
                .is_none()
                .then_some(params.reviewer.as_ref())
                .flatten(),
        )?;
        let (directories, base_branches) = selections(&workspace, &params)?;
        let watch = matches!(actor, Actor::User)
            || self.watch_agent_filed_tasks
            || matches!(params.reviewer, Some(ReviewReviewer::User));
        let sender = actor
            .agent_id()
            .map(|agent_id| {
                let entity_id = self.agent_of_this_project(&workspace.project_id, agent_id)?;
                self.mcp_conversation_generation(&entity_id, agent_id)
            })
            .transpose()?;
        let hooks = LiveOpeningHooks {
            shared: self.self_handle.clone(),
            registry_root: self.workspaces.root().into(),
            project_id: workspace.project_id.clone(),
            project_path: project_path.clone(),
            request_id: params.request_id.clone(),
            store: store.clone(),
            sender,
        };
        let job = OpeningJob {
            store,
            project_id: workspace.project_id.clone(),
            hooks,
            watch,
            request: OpenReviewRequest {
                project_path,
                request_id: params.request_id,
                receiver_root: self.state_root.join("review-receivers"),
                request: ReviewOpeningRequest {
                    workspace_id: params.workspace_id,
                    title: params.title,
                    description: params.description,
                    creator: actor,
                    reviewer: params.reviewer.map(|reviewer| reviewer.assignee()),
                    directories,
                    base_branches,
                },
                workspace,
            },
        };
        self.deferred_work = Some(DeferredWork::Git(Box::new(DeferredGit {
            call: Box::new(job),
            params: Value::Null,
            invalidates: true,
            #[cfg(test)]
            gate: self.off_lock_gate.clone(),
        })));
        Ok(Value::Null)
    }

    fn settle_review_open_watch(
        &mut self,
        project_id: &str,
        task_id: &str,
        watch: bool,
        actor: &Actor,
    ) -> Result<(), String> {
        let (_, task) = self.tracker_task(task_id)?;
        if !watch || task.watched {
            return Ok(());
        }
        let timeline = self
            .tracker_store()?
            .load_tracker_timeline(task_id)
            .map_err(|error| error.to_string())?;
        // An explicit later Unwatch remains authoritative on a lost-response retry.
        if timeline.iter().any(|entry| {
            matches!(entry, TimelineEntry::Event(event)
            if matches!(event.kind, TaskEventKind::Watched | TaskEventKind::Unwatched))
        }) {
            return Ok(());
        }
        self.set_watching(project_id, task, true, actor.clone())?;
        Ok(())
    }
}

impl DeferredGitWork for OpeningJob {
    fn run(&self, _: &Value) -> Result<Value, String> {
        self.publish()
            .and_then(opened_value)
            .map_err(|error| self.error(error))
    }
    fn settle(&self, app: &mut AppState, result: Value) -> Result<Value, String> {
        let task_id = result["task"]["id"]
            .as_str()
            .ok_or("published task has no identity")?
            .to_owned();
        app.workspaces.reload().map_err(|error| self.error(error))?;
        app.settle_review_open_watch(
            &self.project_id,
            &task_id,
            self.watch,
            &self.request.request.creator,
        )
        .map_err(|error| self.error(error))?;
        let mut result = if self.hooks.shared.is_none() {
            let hooks = SettlementHooks {
                app: RefCell::new(app),
                job: self,
            };
            let opened = crate::reviews::opening::retry_reviewer_dispatch(
                &self.store,
                &self.request,
                &hooks,
            )
            .map_err(|error| self.error(error))?;
            opened_value(opened)?
        } else {
            result
        };
        result["task"] = serde_json::to_value(
            self.store
                .load_tracker_task(&task_id)
                .map_err(|error| self.error(error.to_string()))?
                .ok_or("published task disappeared")?,
        )
        .map_err(|error| self.error(error.to_string()))?;
        Ok(result)
    }
    fn invalidate(&self, app: &mut AppState) {
        let _ = app.workspaces.reload();
        app.note_board_changed();
        if let Ok(Some(opening)) = self
            .store
            .load_review_opening(&self.request.project_path, &self.request.request_id)
        {
            app.note_tasks_changed(&self.project_id, &opening.task.id);
        }
    }
    fn invalidates_on_error(&self) -> bool {
        true
    }
}

impl OpeningJob {
    fn publish(&self) -> Result<OpenedReview, String> {
        let retry_dispatch = self
            .store
            .load_review_opening(&self.request.project_path, &self.request.request_id)
            .map_err(|error| error.to_string())?
            .is_some_and(|opening| opening.state == ReviewOpeningState::Published);
        let opened = crate::reviews::opening::open(&self.store, &self.request, &self.hooks)?;
        if retry_dispatch {
            return crate::reviews::opening::retry_reviewer_dispatch(
                &self.store,
                &self.request,
                &self.hooks,
            );
        }
        Ok(opened)
    }

    fn error(&self, message: String) -> String {
        let directories = &self.request.workspace.directories;
        let directory = directories
            .iter()
            .find(|directory| message.contains(&directory.id))
            .or_else(|| (directories.len() == 1).then(|| &directories[0]));
        opening_error(
            message,
            &self.request.workspace.id,
            directory.map(|directory| directory.id.as_str()),
        )
    }
}

fn opening_error(message: String, workspace_id: &str, directory_id: Option<&str>) -> String {
    let mut details = json!({"workspace_id":workspace_id,"recovery":"Restore the selected workspace and source placement, then retry the same request_id to resume or read its published result."});
    if let Some(directory_id) = directory_id {
        details["directory_id"] = json!(directory_id);
    }
    errors::service(message, details)
}

fn opened_value(opened: OpenedReview) -> Result<Value, String> {
    let push_instructions = opened
        .review
        .bindings
        .iter()
        .map(|binding| ReviewPushInstruction {
            directory_id: binding.directory_id.clone(),
            remote: binding.remote_name.clone(),
            branch: binding
                .dedicated_branch_ref
                .strip_prefix("refs/heads/")
                .unwrap_or(&binding.dedicated_branch_ref)
                .to_owned(),
            refspec: format!("HEAD:{}", binding.receiving_ref),
        })
        .collect();
    serde_json::to_value(ReviewOpenResult {
        task: opened.task,
        review: opened.review,
        opening_state: ReviewOpeningState::Published,
        reviewer_dispatch: opened.reviewer_dispatch,
        push_instructions,
    })
    .map_err(|error| error.to_string())
}
