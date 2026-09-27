//! Durable names for agents a task mentions. A Done workspace loses its
//! conversation, so these facts live on the task before that happens.

use super::StoredAnswer;
use crate::app::AppState;
use crate::store::{PersistedPlan, PersistedRun};
use crate::tracker::{Actor, Task, TaskAgentIdentity, TimelineEntry};
use std::cell::OnceCell;
use std::collections::{BTreeSet, HashMap};

/// The store's word on agents no live roster holds, read at most once however
/// many tasks one call resolves identities for.
///
/// Telling who a departed agent was reads every run and every plan, each with
/// its agents' conversation tails. Done again for every agent every task
/// names, that read held the app lock for seconds at a time under
/// `tasks.list` (#128); looking each agent up by restoring every stored
/// roster — each a copy of those tails — held it for another 450 ms on a real
/// store (#131). So the records are read once, without their conversations,
/// into an index of who every stored agent was. Nothing a call does
/// between two tasks changes those records — a backfill writes the task,
/// never a run — so the index stands for the rest of the call. Build one per
/// call, never keep one.
#[derive(Default)]
pub(super) struct StoredRosters {
    known: OnceCell<Option<HashMap<String, TaskAgentIdentity>>>,
}

fn mentioned(task: &Task, timeline: &[TimelineEntry]) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    add_actor(&mut ids, &task.created_by);
    collect_written_agent_ids(&task.title, &mut ids);
    collect_written_agent_ids(&task.body, &mut ids);
    if let Some(id) = task
        .assignee
        .as_ref()
        .and_then(|assignee| assignee.agent_id())
    {
        ids.insert(id.to_string());
    }
    ids.extend(task.trackers.iter().cloned());
    for entry in timeline {
        match entry {
            TimelineEntry::Comment(comment) => {
                add_actor(&mut ids, &comment.author);
                collect_written_agent_ids(&comment.body, &mut ids);
            }
            TimelineEntry::Event(event) => {
                add_actor(&mut ids, &event.actor);
                collect_payload_agent_ids(&event.payload, &mut ids);
            }
        }
    }
    ids
}

/// Explicit `@agent:<id>` references in task prose name agents even when
/// they never acted on that task. Agent ids contain letters, digits, `-` and
/// `_`; stopping there also excludes sentence punctuation from the id.
fn collect_written_agent_ids(text: &str, ids: &mut BTreeSet<String>) {
    for part in text.split("@agent:").skip(1) {
        let id: String = part
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_')
            .collect();
        if id.starts_with("agent-") || id.starts_with("project-") {
            ids.insert(id);
        }
    }
}

/// Assignment history uses two payload shapes: `assignee.agent_id` and
/// `dispatched.agent_id`. Walking object keys keeps both, including future
/// event kinds that carry the same explicit agent reference.
fn collect_payload_agent_ids(value: &serde_json::Value, ids: &mut BTreeSet<String>) {
    match value {
        serde_json::Value::Object(fields) => {
            if let Some(id) = fields.get("agent_id").and_then(serde_json::Value::as_str) {
                ids.insert(id.to_string());
            }
            for child in fields.values() {
                collect_payload_agent_ids(child, ids);
            }
        }
        serde_json::Value::Array(items) => {
            for child in items {
                collect_payload_agent_ids(child, ids);
            }
        }
        _ => {}
    }
}

fn add_actor(ids: &mut BTreeSet<String>, actor: &Actor) {
    if let Some(id) = actor.agent_id() {
        ids.insert(id.to_string());
    }
}

fn retain_known_fields(prior: &TaskAgentIdentity, current: TaskAgentIdentity) -> TaskAgentIdentity {
    if current.available {
        return current;
    }
    TaskAgentIdentity {
        agent_id: current.agent_id,
        name: current.name.or_else(|| prior.name.clone()),
        ordinal: current.ordinal.or(prior.ordinal),
        workspace_id: current.workspace_id.or_else(|| prior.workspace_id.clone()),
        workspace_name: current
            .workspace_name
            .or_else(|| prior.workspace_name.clone()),
        provider: current.provider.or_else(|| prior.provider.clone()),
        available: false,
    }
}

/// Every agent the stored plans name, as the identity a task shows.
fn plan_identities(plans: &[PersistedPlan]) -> impl Iterator<Item = TaskAgentIdentity> + '_ {
    plans.iter().flat_map(|plan| {
        plan.members().into_iter().map(|member| TaskAgentIdentity {
            agent_id: member.id,
            name: member.name,
            ordinal: Some(member.ordinal),
            workspace_id: None,
            workspace_name: None,
            provider: Some(member.provider.wire_id().to_string()),
            available: false,
        })
    })
}

impl AppState {
    /// Migrate every task before a roster can disappear. An old author's
    /// only reference may be in a closed, unlinked task's timeline; reads
    /// and the workspace's automatic close operation cannot cover that case.
    /// A failed save refuses removal while the source records still exist.
    pub(in crate::app) fn preserve_project_task_identities(
        &self,
        project_id: &str,
    ) -> Result<(), String> {
        let Some(store) = &self.store else {
            return Ok(());
        };
        let project_path = self.tracker_project_path(project_id)?;
        let tasks = store
            .list_tracker_tasks(&project_path, crate::store::TaskFilter::default())
            .stored()?;
        let ids: Vec<String> = tasks.iter().map(|task| task.id.clone()).collect();
        let mut timelines = store.load_tracker_timelines(&ids).stored()?;
        let rosters = StoredRosters::default();
        for task in tasks {
            let timeline = timelines.remove(&task.id).unwrap_or_default();
            self.backfill_task_identities(task, &timeline, &rosters)?;
        }
        Ok(())
    }

    pub(in crate::app) fn preserve_entity_task_identities(
        &self,
        entity_id: &str,
    ) -> Result<(), String> {
        if let Some(project_id) = self.projects.project_id_of(entity_id) {
            self.preserve_project_task_identities(project_id)?;
        }
        Ok(())
    }

    /// Resolve the invalidation before changing the roster: a store failure
    /// must not remove an agent and then leave other clients' task caches
    /// pointing at it. Preservation has already filled historical identities,
    /// including those on closed tasks with no workspace link.
    pub(in crate::app) fn tasks_with_agent_identity(
        &self,
        entity_id: &str,
        agent_id: &str,
    ) -> Result<Option<(String, Vec<String>)>, String> {
        let Some(project_id) = self.projects.project_id_of(entity_id) else {
            return Ok(None);
        };
        let Some(store) = &self.store else {
            return Ok(None);
        };
        let project_path = self.tracker_project_path(project_id)?;
        let task_ids: Vec<_> = store
            .list_tracker_tasks(&project_path, crate::store::TaskFilter::default())
            .stored()?
            .into_iter()
            .filter(|task| task.identities.contains_key(agent_id))
            .map(|task| task.id)
            .collect();
        Ok((!task_ids.is_empty()).then(|| (project_id.to_string(), task_ids)))
    }

    pub(super) fn task_json_with_live_identities(
        &self,
        project_id: &str,
        task: &Task,
    ) -> serde_json::Value {
        let task = self.task_with_read_identities(task.clone(), &[], &StoredRosters::default());
        super::task_json(project_id, &task)
    }

    fn live_task_identity(&self, agent_id: &str) -> Option<TaskAgentIdentity> {
        let entity_id = self.entity_of_agent(agent_id)?;
        let agent = self.entity_agents(&entity_id).ok()?.by_id(agent_id)?;
        let workspace = if crate::agent::is_project_agent(agent_id) {
            None
        } else {
            let root = self.runs.get(&entity_id)?.worktree.path.clone();
            let project_id = self.projects.project_id_of(&entity_id)?;
            self.workspaces
                .list(Some(project_id))
                .into_iter()
                .find(|workspace| {
                    crate::app::workspaces::same_path(&workspace.root, &root)
                        && self.workspace_conversation_owner(workspace).as_deref()
                            == Some(&entity_id)
                        && self.projects.get(project_id).is_some_and(|project| {
                            !crate::app::workspaces::same_path(&workspace.root, &project.repo_path)
                        })
                })
        };
        let available = workspace
            .is_some_and(|workspace| workspace.status == crate::workspace::WorkspaceStatus::Ready)
            || crate::agent::is_project_agent(agent_id);
        Some(TaskAgentIdentity {
            agent_id: agent_id.to_string(),
            name: agent.name.clone(),
            ordinal: Some(agent.ordinal),
            workspace_id: workspace.map(|workspace| workspace.id.clone()),
            workspace_name: workspace.map(|workspace| workspace.name.clone()),
            provider: Some(agent.choice.provider.wire_id().to_string()),
            available,
        })
    }

    /// A run can have left the active roster while its agent row still lives
    /// in the store. Read that row before calling an older actor unknown.
    fn stored_task_identity(
        &self,
        agent_id: &str,
        rosters: &StoredRosters,
    ) -> Option<TaskAgentIdentity> {
        rosters
            .known
            .get_or_init(|| self.stored_identities())
            .as_ref()?
            .get(agent_id)
            .cloned()
    }

    /// Who every stored agent was: runs first, then plans, the first record
    /// naming an agent answering for it. A store whose runs cannot be read
    /// knows nobody.
    fn stored_identities(&self) -> Option<HashMap<String, TaskAgentIdentity>> {
        let store = self.tracker_store().ok()?;
        let runs = store.load_all_run_rosters().ok()?;
        let plans = store.load_all_plan_rosters().unwrap_or_default();
        let mut known = HashMap::new();
        for identity in self.run_identities(&runs).chain(plan_identities(&plans)) {
            known.entry(identity.agent_id.clone()).or_insert(identity);
        }
        Some(known)
    }

    /// Every agent the stored runs name, with the workspace each run's
    /// checkout still is, if it is one.
    fn run_identities<'a>(
        &'a self,
        runs: &'a [PersistedRun],
    ) -> impl Iterator<Item = TaskAgentIdentity> + 'a {
        runs.iter().flat_map(|run| {
            let workspace = self.workspaces.list(None).into_iter().find(|workspace| {
                crate::app::workspaces::same_path(
                    &workspace.root,
                    std::path::Path::new(&run.worktree_path),
                )
            });
            let workspace_id = workspace.map(|workspace| workspace.id.clone());
            run.members()
                .into_iter()
                .map(move |member| TaskAgentIdentity {
                    agent_id: member.id,
                    name: member.name,
                    ordinal: Some(member.ordinal),
                    workspace_id: workspace_id.clone(),
                    workspace_name: Some(run.worktree_name.clone()),
                    provider: Some(member.provider.wire_id().to_string()),
                    available: false,
                })
        })
    }

    fn resolved_task_identity(
        &self,
        agent_id: &str,
        rosters: &StoredRosters,
    ) -> Option<TaskAgentIdentity> {
        self.live_task_identity(agent_id)
            .or_else(|| self.stored_task_identity(agent_id, rosters))
    }

    /// Add the identities this write can still inspect before a workspace is
    /// removed. Existing snapshots stay when an agent is no longer present.
    pub(super) fn capture_task_identities(&self, task: &mut Task, timeline: &[TimelineEntry]) {
        self.capture_identities_with(task, timeline, &StoredRosters::default());
    }

    fn capture_identities_with(
        &self,
        task: &mut Task,
        timeline: &[TimelineEntry],
        rosters: &StoredRosters,
    ) {
        for id in mentioned(task, timeline) {
            if let Some(identity) = self.resolved_task_identity(&id, rosters) {
                task.identities
                    .entry(id)
                    .and_modify(|saved| {
                        *saved = retain_known_fields(saved, identity.clone());
                    })
                    .or_insert(identity);
            }
        }
    }

    /// Backfill all historical actors while their records can still be read.
    /// A read is a durable migration: the next workspace removal must not
    /// turn an older comment's author back into an opaque id.
    pub(super) fn backfill_task_identities(
        &self,
        mut task: Task,
        timeline: &[TimelineEntry],
        rosters: &StoredRosters,
    ) -> Result<Task, String> {
        let before = task.identities.clone();
        self.capture_identities_with(&mut task, timeline, rosters);
        if task.identities != before {
            self.tracker_store()?
                .save_tracker_task_activity(&task, &[], &[])
                .stored()?;
        }
        Ok(task)
    }

    /// Fill old tasks from living agents and mark departed ones unavailable.
    /// Missing agents still get a row, so the client has an honest fallback.
    pub(super) fn task_with_read_identities(
        &self,
        mut task: Task,
        timeline: &[TimelineEntry],
        rosters: &StoredRosters,
    ) -> Task {
        let ids = mentioned(&task, timeline);
        for id in ids {
            if let Some(identity) = self.resolved_task_identity(&id, rosters) {
                task.identities
                    .entry(id)
                    .and_modify(|saved| {
                        *saved = retain_known_fields(saved, identity.clone());
                    })
                    .or_insert(identity);
            } else {
                task.identities
                    .entry(id.clone())
                    .or_insert_with(|| TaskAgentIdentity {
                        agent_id: id,
                        name: None,
                        ordinal: None,
                        workspace_id: None,
                        workspace_name: None,
                        provider: None,
                        available: false,
                    });
            }
        }
        for identity in task.identities.values_mut() {
            identity.available = self
                .live_task_identity(&identity.agent_id)
                .is_some_and(|live| live.available);
        }
        task
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn assignment_history_keeps_agent_ids_after_reassignment() {
        let mut task = Task::drafted("/repo", "one", Actor::User, "2026-01-01T00:00:00Z");
        task.assignee = Some(crate::tracker::Assignee::User);
        let assigned = crate::tracker::TaskEvent::new(
            &task.id,
            Actor::User,
            crate::tracker::TaskEventKind::Assigned,
            json!({ "assignee": { "kind": "agent", "agent_id": "agent-before" } }),
            "2026-01-01T00:00:00Z",
        );
        let dispatched = crate::tracker::TaskEvent::new(
            &task.id,
            Actor::User,
            crate::tracker::TaskEventKind::Dispatched,
            json!({ "agent_id": "agent-after" }),
            "2026-01-01T00:00:01Z",
        );
        let ids = mentioned(
            &task,
            &[
                TimelineEntry::Event(assigned),
                TimelineEntry::Event(dispatched),
            ],
        );
        assert_eq!(
            ids,
            BTreeSet::from(["agent-before".to_string(), "agent-after".to_string()])
        );
    }

    #[test]
    fn prose_reference_keeps_an_agent_who_never_acted() {
        let mut task = Task::drafted("/repo", "one", Actor::User, "2026-01-01T00:00:00Z");
        task.body = "Ask @agent:agent-4. Then check @agent:project-7".into();
        assert_eq!(
            mentioned(&task, &[]),
            BTreeSet::from(["agent-4".to_string(), "project-7".to_string()])
        );
    }
}
