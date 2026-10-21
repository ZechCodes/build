//! Task reads combine one SQL snapshot with a small, immutable live overlay.
use super::{edits, pages, read_source::TaskReadSource, views, StoredAnswer};
use crate::app::{require_str, AppState};
use crate::session_summary::UserSession;
use crate::store::{TaskFilter, TaskSeek};
use crate::tracker::{Task, TimelineEntry};
use serde_json::{json, Value};

#[derive(Clone, Default)]
pub(in crate::app) struct TaskReadContext {
    pub(super) project_paths: imbl::OrdMap<String, String>,
    pub(super) identities: super::read_identities::TaskIdentityContext,
    user_session: UserSession,
}

#[derive(Clone)]
pub(in crate::app) struct TaskReadFence {
    project_id: String,
    project_path: String,
}

impl TaskReadFence {
    pub(in crate::app) fn project_id(&self) -> &str {
        &self.project_id
    }
}

impl AppState {
    /// Mixed observations need project admission only, without identity path probes.
    pub(in crate::app) fn task_scope_read_context(&self) -> TaskReadContext {
        TaskReadContext {
            project_paths: self
                .projects
                .iter()
                .map(|project| (project.id.clone(), project.repo_path.display().to_string()))
                .collect(),
            identities: Default::default(),
            user_session: self.user_session,
        }
    }
    /// Producer-only capture. Path aliases are resolved here, never by a read.
    pub(in crate::app) fn task_read_context(&self) -> TaskReadContext {
        TaskReadContext {
            project_paths: self
                .projects
                .iter()
                .map(|project| (project.id.clone(), project.repo_path.display().to_string()))
                .collect(),
            identities: self.task_identity_context(),
            user_session: self.user_session,
        }
    }
}

impl TaskReadContext {
    #[cfg(test)]
    pub(in crate::app) fn test_user_session(&self) -> UserSession {
        self.user_session
    }

    pub(in crate::app) fn set_user_session(&mut self, session: UserSession) {
        self.user_session = session;
    }

    pub(in crate::app) fn update_scope_project(&mut self, id: &str, path: Option<String>) {
        match path {
            Some(path) => {
                self.project_paths.insert(id.into(), path);
            }
            None => {
                self.project_paths.remove(id);
            }
        }
    }
    /// Only a successful owner save can admit a new live agent. Owners checked
    /// out by a surrounding mutation retain their last published observation.
    pub(in crate::app) fn retain_live_owners(
        &mut self,
        previous: &Self,
        app: &AppState,
        published: Option<(&str, bool)>,
    ) {
        self.identities
            .retain_live_owners(&previous.identities, app, published);
    }

    pub(in crate::app) fn retain_saved_owners(
        &mut self,
        previous: &Self,
        app: &AppState,
        owners: &std::collections::HashSet<String>,
    ) {
        self.identities
            .retain_saved_owners(&previous.identities, app, owners);
    }

    pub(in crate::app) fn forget_live_owner(&mut self, owner: &str) {
        self.identities.forget_live_owner(owner);
    }

    pub(in crate::app) fn validate_scope(
        &self,
        source: &impl TaskReadSource,
        method: &str,
        params: &Value,
    ) -> Result<TaskReadFence, String> {
        let project_id = if method == "tasks.list" {
            require_str(params, "project_id")?
        } else {
            let id = require_str(params, "task_id")?;
            self.tracker_task(source, &id)?.0
        };
        let project_path = self
            .project_paths
            .get(&project_id)
            .cloned()
            .ok_or_else(|| format!("unknown project_id: {project_id}"))?;
        Ok(TaskReadFence {
            project_id,
            project_path,
        })
    }

    pub(in crate::app) fn validate_fence(&self, fence: &TaskReadFence) -> Result<(), String> {
        if self.project_paths.get(&fence.project_id) == Some(&fence.project_path) {
            Ok(())
        } else {
            Err(format!("unknown project_id: {}", fence.project_id))
        }
    }

    pub(in crate::app) fn tasks_list(
        &self,
        source: &impl TaskReadSource,
        params: &Value,
    ) -> Result<Value, String> {
        let project_id = require_str(params, "project_id")?;
        let project_path = self
            .project_paths
            .get(&project_id)
            .ok_or_else(|| format!("unknown project_id: {project_id}"))?;
        let state = edits::optional_state(params)?;
        let status = edits::optional_status(params, "status")?;
        let assignee = edits::optional_assignee_filter(params)?;
        let label = crate::app::optional_nonempty_string(params, "label")?;
        let key = source.tracker_list_key().stored()?;
        let page = pages::PageAsk::parse(
            params,
            &pages::ListOf {
                store_key: &key,
                project_path,
            },
            &pages::ListFilter {
                state,
                status: status.as_deref(),
                assignee: &assignee,
                label,
            },
        )?;
        let stretch = source
            .list_filtered_tasks(
                project_path,
                TaskFilter {
                    state,
                    status: status.as_deref(),
                },
                TaskSeek {
                    below: page.below,
                    take: page.rows_to_keep(),
                    scan: page.rows_to_scan(),
                },
                |task, target| assignee.matches(task, target) && edits::carries_label(task, label),
            )
            .stored()?;
        let mut tasks = stretch.tasks;
        let next_cursor = page.cut(&mut tasks, stretch.scanned_to, |task| task.number);
        let rows = self.listed_rows(source, &project_id, tasks)?;
        let mut answer =
            json!({"project_id":project_id,"tasks":rows,"user_session":self.user_session_json()});
        if let Some(cursor) = next_cursor {
            answer["next_cursor"] = json!(cursor);
        }
        Ok(answer)
    }

    fn listed_rows(
        &self,
        source: &impl TaskReadSource,
        project_id: &str,
        tasks: Vec<Task>,
    ) -> Result<Vec<Value>, String> {
        let ids = tasks.iter().map(|task| task.id.clone()).collect::<Vec<_>>();
        let mut timelines = source.load_tracker_timelines(&ids).stored()?;
        let rosters = super::read_identities::ReadRosters::default();
        tasks
            .into_iter()
            .map(|task| {
                let timeline = timelines.remove(&task.id).unwrap_or_default();
                let task = self.read_task(source, task, &timeline, &rosters);
                let mut row = views::read_task_json(project_id, &task, &timeline);
                extend_task_summary(source, &task, &mut row)?;
                Ok(row)
            })
            .collect()
    }

    pub(in crate::app) fn tasks_get(
        &self,
        source: &impl TaskReadSource,
        params: &Value,
    ) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        let (project, task) = self.tracker_task(source, &task_id)?;
        let timeline = source.load_tracker_timeline(&task_id).stored()?;
        let task = self.read_task(source, task, &timeline, &Default::default());
        let mut answer = views::task_with_timeline_json(&project, &task, &timeline);
        extend_task_summary(source, &task, &mut answer["task"])?;
        Ok(answer)
    }

    fn read_task(
        &self,
        source: &impl TaskReadSource,
        mut task: Task,
        timeline: &[TimelineEntry],
        rosters: &super::read_identities::ReadRosters,
    ) -> Task {
        if task.done_at.is_none() {
            task.done_at = crate::tracker::done_at_from_timeline(&task, timeline);
        }
        self.identities.read_task(source, task, timeline, rosters)
    }

    pub(in crate::app) fn tasks_waiting_on_blocks(
        &self,
        source: &impl TaskReadSource,
        params: &Value,
    ) -> Result<Value, String> {
        let id = require_str(params, "task_id")?;
        let (_, task) = self.tracker_task(source, &id)?;
        let limit = match params.get("limit") {
            None => 20,
            Some(value) => value
                .as_u64()
                .filter(|n| (1..=100).contains(n))
                .ok_or("limit must be 1 to 100")? as usize,
        };
        let cursor = crate::app::optional_nonempty_string(params, "cursor")?;
        let page = source
            .waiting_on_blocks(&task.project_path, &id, limit, cursor)
            .stored()?;
        let mut answer = json!({"blocks":page.blocks});
        if let Some(cursor) = page.next_cursor {
            answer["next_cursor"] = json!(cursor);
        }
        Ok(answer)
    }

    fn tracker_task(
        &self,
        source: &impl TaskReadSource,
        id: &str,
    ) -> Result<(String, Task), String> {
        let task = source
            .load_tracker_task(id)
            .stored()?
            .ok_or_else(|| format!("unknown task_id: {id}"))?;
        let project = self
            .project_paths
            .iter()
            .find(|(_, path)| **path == task.project_path)
            .map(|(id, _)| id.clone())
            .ok_or_else(|| format!("unknown task_id: {id}"))?;
        Ok((project, task))
    }

    fn user_session_json(&self) -> Value {
        let session = self.user_session;
        json!({
            "session_started_ms":session.session_started_ms,
            "last_activity_ms":session.last_activity_ms,
            "previous_session_ended_ms":session.previous_session_ended_ms,
            "gap_ms":crate::session_summary::USER_SESSION_GAP_MS,
            "now_ms":i64::try_from(crate::agent::now_ms()).unwrap_or(i64::MAX),
        })
    }
}

fn extend_task_summary(
    source: &impl TaskReadSource,
    task: &Task,
    row: &mut Value,
) -> Result<(), String> {
    if let Some(review) = source.load_review_summary(&task.id).stored()? {
        row["review_summary"] = json!(review);
    }
    row["dependencies"] = json!(source
        .waiting_on_summary(&task.project_path, &task.id)
        .stored()?);
    let page = source
        .waiting_on_blocks(&task.project_path, &task.id, 20, None)
        .stored()?;
    row["dependencies"]["blocks"] = json!(page.blocks);
    if let Some(cursor) = page.next_cursor {
        row["dependencies"]["blocks_next_cursor"] = json!(cursor);
    }
    extend_assignment(source, &task.id, row)
}

fn extend_assignment(
    source: &impl TaskReadSource,
    id: &str,
    row: &mut Value,
) -> Result<(), String> {
    let Some(intent) = source
        .current_delivery_intent(id)
        .stored()?
        .filter(|intent| {
            matches!(
                intent.state.as_str(),
                "waiting" | "queued" | "provisioning" | "failed"
            )
        })
    else {
        return Ok(());
    };
    let state = match intent.state.as_str() {
        "provisioning" => "queued",
        state => state,
    };
    row["deferred_assignment"] =
        json!({"id":intent.id,"target":intent.target,"state":state,"error":intent.error});
    Ok(())
}
