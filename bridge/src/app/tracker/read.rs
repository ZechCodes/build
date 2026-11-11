//! Task reads combine one SQL snapshot with a small, immutable live overlay.
use super::{edits, pages, read_source::TaskReadSource, views, StoredAnswer};
use crate::app::{require_str, AppState};
use crate::session_summary::UserSession;
use crate::store::{TaskFilter, TaskSeek};
use crate::tracker::{Task, TimelineEntry};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone, Default)]
pub(in crate::app) struct TaskReadContext {
    pub(super) project_paths: imbl::OrdMap<String, String>,
    project_ids: Arc<BTreeMap<String, String>>,
    sync_project_paths: imbl::OrdMap<String, String>,
    pub(super) identities: super::read_identities::TaskIdentityContext,
    user_session: UserSession,
    deleting: std::collections::HashSet<String>,
}

#[derive(Clone)]
pub(in crate::app) struct TaskReadFence {
    project_id: String,
    project_path: String,
}

pub(in crate::app) enum TaskSyncAdmission {
    Active(super::sync_scopes::TaskSyncScope, TaskReadFence),
    Removed(super::sync_scopes::TaskSyncScope, String),
    Unavailable(String),
}

impl TaskReadFence {
    pub(in crate::app) fn project_id(&self) -> &str {
        &self.project_id
    }
}

impl AppState {
    fn task_project_maps(&self) -> (imbl::OrdMap<String, String>, Arc<BTreeMap<String, String>>) {
        let paths: imbl::OrdMap<String, String> = self
            .projects
            .iter()
            .map(|project| (project.id.clone(), project.project_key.clone()))
            .collect();
        let ids = paths
            .iter()
            .map(|(id, path)| (path.clone(), id.clone()))
            .collect();
        (paths, Arc::new(ids))
    }

    /// Mixed observations need project admission only, without identity path probes.
    pub(in crate::app) fn task_scope_read_context(&self) -> TaskReadContext {
        let (project_paths, project_ids) = self.task_project_maps();
        TaskReadContext {
            sync_project_paths: project_paths.clone(),
            project_paths,
            project_ids,
            identities: Default::default(),
            user_session: self.user_session,
            deleting: self.deletion_pending_addresses(),
        }
    }
    /// Producer-only capture. Path aliases are resolved here, never by a read.
    pub(in crate::app) fn task_read_context(&self) -> TaskReadContext {
        let (project_paths, project_ids) = self.task_project_maps();
        TaskReadContext {
            sync_project_paths: project_paths.clone(),
            project_paths,
            project_ids,
            identities: self.task_identity_context(),
            user_session: self.user_session,
            deleting: self.deletion_pending_addresses(),
        }
    }
}

impl TaskReadContext {
    pub(in crate::app) fn sync_scope(
        &self,
        source: &impl TaskReadSource,
        asked: &crate::api::v1::sync::Scope,
    ) -> Result<TaskSyncAdmission, String> {
        match asked {
            crate::api::v1::sync::Scope::Device => {
                Err("device sync requires its independent recovery adapter".into())
            }
            crate::api::v1::sync::Scope::ProjectTasks {
                project_id,
                filters,
                order,
            } => {
                self.refuse_deletion_read(source, project_id)?;
                let mut query = serde_json::to_value(filters.clone().unwrap_or_default())
                    .map_err(|error| error.to_string())?;
                if let Some(order) = order {
                    query["order"] = serde_json::to_value(order).map_err(|e| e.to_string())?;
                }
                let project_path = self
                    .sync_project_paths
                    .get(project_id)
                    .cloned()
                    .unwrap_or_default();
                let scope =
                    super::sync_scopes::TaskSyncScope::project(project_id, &project_path, &query)?
                        .with_project_ids(Arc::clone(&self.project_ids));
                if self.project_paths.contains_key(project_id) {
                    Ok(TaskSyncAdmission::Active(
                        scope,
                        TaskReadFence {
                            project_id: project_id.clone(),
                            project_path,
                        },
                    ))
                } else {
                    Ok(TaskSyncAdmission::Removed(
                        scope,
                        format!("unknown project_id: {project_id}"),
                    ))
                }
            }
            crate::api::v1::sync::Scope::Task { task_id } => {
                let Some((project_path, summary_project)) =
                    source.task_project_authority(task_id).stored()?
                else {
                    return Ok(TaskSyncAdmission::Removed(
                        super::sync_scopes::TaskSyncScope::task(task_id, "", "")
                            .with_project_ids(Arc::clone(&self.project_ids)),
                        format!("unknown task_id: {task_id}"),
                    ));
                };
                self.refuse_deletion_read(source, &project_path)?;
                let Some(project_id) = self.project_ids.get(&project_path).cloned() else {
                    return Ok(TaskSyncAdmission::Removed(
                        super::sync_scopes::TaskSyncScope::task(task_id, "", "")
                            .with_project_ids(Arc::clone(&self.project_ids)),
                        format!("unknown task_id: {task_id}"),
                    ));
                };
                let Some(summary_project) = summary_project else {
                    return Ok(TaskSyncAdmission::Unavailable(
                        "task projection is incomplete".into(),
                    ));
                };
                if summary_project != project_path {
                    return Ok(TaskSyncAdmission::Unavailable(
                        "task projection does not match canonical authority".into(),
                    ));
                }
                let scope =
                    super::sync_scopes::TaskSyncScope::task(task_id, &project_id, &project_path)
                        .with_project_ids(Arc::clone(&self.project_ids));
                Ok(TaskSyncAdmission::Active(
                    scope,
                    TaskReadFence {
                        project_id,
                        project_path,
                    },
                ))
            }
        }
    }

    pub(in crate::app) fn set_user_session(&mut self, session: UserSession) {
        self.user_session = session;
    }

    pub(in crate::app) fn update_scope_project(&mut self, id: &str, path: Option<String>) {
        match path {
            Some(path) => {
                self.project_paths.insert(id.into(), path.clone());
                self.sync_project_paths.insert(id.into(), path.clone());
                Arc::make_mut(&mut self.project_ids).insert(path, id.into());
            }
            None => {
                if let Some(path) = self.project_paths.remove(id) {
                    Arc::make_mut(&mut self.project_ids).remove(&path);
                }
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
        self.retain_sync_projects(previous);
        self.identities
            .retain_live_owners(&previous.identities, app, published);
    }

    pub(in crate::app) fn retain_saved_owners(
        &mut self,
        previous: &Self,
        app: &AppState,
        owners: &std::collections::HashSet<String>,
    ) {
        self.retain_sync_projects(previous);
        self.identities
            .retain_saved_owners(&previous.identities, app, owners);
    }

    pub(in crate::app) fn forget_live_owner(&mut self, owner: &str) {
        self.identities.forget_live_owner(owner);
    }

    fn retain_sync_projects(&mut self, previous: &Self) {
        for (id, path) in &previous.sync_project_paths {
            if !self.sync_project_paths.contains_key(id) {
                self.sync_project_paths.insert(id.clone(), path.clone());
            }
        }
    }

    pub(in crate::app) fn validate_scope(
        &self,
        source: &impl TaskReadSource,
        method: &str,
        params: &Value,
    ) -> Result<TaskReadFence, String> {
        let identity = match params["project_id"].as_str() {
            Some(id) => id.to_string(),
            None => source
                .task_project_authority(params["task_id"].as_str().unwrap_or_default())
                .stored()?
                .map(|(key, _)| key)
                .unwrap_or_default(),
        };
        self.refuse_deletion_read(source, &identity)?;
        let project_id = if matches!(
            method,
            "tasks.list" | "tasks.filterChoices" | "tasks.counts" | "tasks.lookup"
        ) {
            require_str(params, "project_id")?
        } else {
            let id = require_str(params, "task_id")?;
            if method == "tasks.get" && super::bounded_detail::requested(params)? {
                let (path, _) = source
                    .task_project_authority(&id)
                    .stored()?
                    .ok_or_else(|| format!("unknown task_id: {id}"))?;
                self.project_ids
                    .get(&path)
                    .cloned()
                    .ok_or_else(|| format!("unknown task_id: {id}"))?
            } else {
                self.tracker_task(source, &id)?.0
            }
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
        if params.get("summary").and_then(Value::as_bool) == Some(true) {
            return self.tasks_list_summary(source, params);
        }
        self.tasks_list_scanned(source, params, None)
    }

    /// MCP bounds candidate decoding as well as returned rows. Sparse filters
    /// can produce an empty page with a cursor; RPC keeps its existing scan policy.
    pub(in crate::app) fn mcp_tasks_list(
        &self,
        source: &impl TaskReadSource,
        params: &Value,
    ) -> Result<Value, String> {
        let limit = match params.get("limit") {
            None | Some(Value::Null) => crate::mcp::MCP_TASK_LIST_MAX,
            Some(value) => value
                .as_u64()
                .filter(|limit| (1..=crate::mcp::MCP_TASK_LIST_MAX).contains(limit))
                .ok_or_else(|| format!("limit must be 1 to {}", crate::mcp::MCP_TASK_LIST_MAX))?,
        };
        let mut params = params.clone();
        params["limit"] = json!(limit);
        self.tasks_list_scanned(source, &params, Some(limit as usize))
    }

    fn tasks_list_scanned(
        &self,
        source: &impl TaskReadSource,
        params: &Value,
        scan: Option<usize>,
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
        let selections = super::selections::has_plural(params)
            .then(|| super::selections::TaskSelections::parse(params))
            .transpose()?;
        let canonical = selections.as_ref().map(|selection| selection.canonical());
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
                selections: canonical.as_ref(),
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
                    scan: scan.or_else(|| page.rows_to_scan()),
                },
                |task, target| match &selections {
                    Some(selections) => selections.matches_task(task, target),
                    None => {
                        assignee.matches(task, target)
                            && label.is_none_or(|label| {
                                task.labels
                                    .iter()
                                    .any(|one| one.eq_ignore_ascii_case(label))
                            })
                    }
                },
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
                #[cfg(test)]
                let _phase = crate::timing::phase_samples::phase("task.row");
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
        if super::bounded_detail::requested(params)? {
            return self.tasks_get_paged(source, params);
        }
        let task_id = require_str(params, "task_id")?;
        let (project, task) = self.tracker_task(source, &task_id)?;
        let timeline = source.load_tracker_timeline(&task_id).stored()?;
        let task = self.read_task(source, task, &timeline, &Default::default());
        let mut answer = views::task_with_timeline_json(&project, &task, &timeline);
        extend_task_summary(source, &task, &mut answer["task"])?;
        let (linked_reviews, truncated) = source
            .load_linked_review_summaries(&task.project_path, &task.id)
            .stored()?;
        if !linked_reviews.is_empty() {
            answer["task"]["linked_reviews"] = json!(linked_reviews);
        }
        if truncated {
            answer["task"]["linked_reviews_truncated"] = json!(true);
        }

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

    pub(super) fn user_session_json(&self) -> Value {
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

pub(super) fn extend_task_summary(
    source: &impl TaskReadSource,
    task: &Task,
    row: &mut Value,
) -> Result<(), String> {
    #[cfg(test)]
    let _phase = crate::timing::phase_samples::phase("task.summary");
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

impl TaskReadContext {
    fn refuse_deletion_read(
        &self,
        source: &impl TaskReadSource,
        identity: &str,
    ) -> Result<(), String> {
        if self.deleting.contains(identity) || source.project_deletion_pending(identity).stored()? {
            return Err("Project is deletion-pending; retry project.delete with the original project_id and deletion mode".into());
        }
        Ok(())
    }
}
