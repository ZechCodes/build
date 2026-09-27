//! Who is watching a task, and what one agent is on (spec: Tasks →
//! Tracking).
//!
//! Tracking is a set on the task and two verbs over it. What tracking is FOR
//! — the notices a change delivers — is [`super::notices`]; this file is only
//! the membership and the per-agent read.

use super::{StoredAnswer, TaskWrite};
use crate::app::{require_str, AppState};
use crate::store::TaskFilter;
use crate::tracker::{Actor, Task, TaskEventKind};
use serde_json::{json, Value};

impl AppState {
    /// `tasks.track` — this agent wants to hear about this task.
    ///
    /// Idempotent, and quiet about it: an agent already watching answers the
    /// task unchanged with no event. A set does not record being told twice.
    pub(crate) fn tasks_track(&mut self, params: &Value) -> Result<Value, String> {
        let (project_id, task, agent_id) = self.tracking_request(params)?;
        self.set_tracking(&project_id, task, &agent_id, true, Actor::User, None)
    }

    /// `tasks.untrack` — stop hearing about it.
    pub(crate) fn tasks_untrack(&mut self, params: &Value) -> Result<Value, String> {
        let (project_id, task, agent_id) = self.tracking_request(params)?;
        self.set_tracking(&project_id, task, &agent_id, false, Actor::User, None)
    }

    /// The task and the agent a tracking call names, both checked against the
    /// project the task belongs to.
    ///
    /// An agent of another project is refused by name, the way every
    /// project-scoped handler refuses one: a task's watchers are its own
    /// project's agents, and an agent elsewhere could not read what it was
    /// told anyway.
    fn tracking_request(&mut self, params: &Value) -> Result<(String, Task, String), String> {
        let task_id = require_str(params, "task_id")?;
        let agent_id = require_str(params, "agent_id")?;
        let (project_id, task) = self.tracker_task(&task_id)?;
        let entity_id = self
            .entity_of_agent(&agent_id)
            .ok_or_else(|| format!("unknown agent_id: {agent_id}"))?;
        if self.projects.project_id_of(&entity_id) != Some(project_id.as_str()) {
            return Err(format!("agent {agent_id} is not in project {project_id}"));
        }
        Ok((project_id, task, agent_id))
    }

    /// Add or remove one tracker, writing the event only when something
    /// actually changed.
    ///
    /// `because` is what put the agent on the list when it was not the agent
    /// asking — `"assignment"`, today — so a timeline reader can tell a
    /// request from a consequence.
    pub(in crate::app) fn set_tracking(
        &mut self,
        project_id: &str,
        task: Task,
        agent_id: &str,
        tracking: bool,
        actor: Actor,
        because: Option<&str>,
    ) -> Result<Value, String> {
        let now = crate::store::now_rfc3339();
        let mut write = TaskWrite::by(actor.clone(), task);
        let changed = if tracking {
            write.task.track(agent_id)?
        } else {
            write.task.untrack(agent_id)
        };
        if !changed {
            // Nothing moved, so nothing is written and nothing is pushed: a
            // set does not record being told twice, and a timeline that said
            // otherwise would be claiming a second fact for one truth.
            return Ok(json!({
                "task": self.task_json_with_live_identities(project_id, &write.task),
            }));
        }
        let mut payload = json!({ "agent_id": agent_id });
        if let (Some(because), Some(payload)) = (because, payload.as_object_mut()) {
            payload.insert("by".to_string(), json!(because));
        }
        let kind = if tracking {
            TaskEventKind::Tracked
        } else {
            TaskEventKind::Untracked
        };
        write.event(&actor, kind, payload, &now);
        self.commit_task_write(project_id, write, &now)
    }

    /// `tasks.watch` — the user wants this task in their inbox.
    ///
    /// Idempotent and quiet about it, like `tasks.track`: watching a second
    /// time is not a second fact, and a timeline that said so would be
    /// claiming two.
    pub(crate) fn tasks_watch(&mut self, params: &Value) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        let (project_id, task) = self.tracker_task(&task_id)?;
        self.set_watching(&project_id, task, true, Actor::User)
    }

    /// `tasks.unwatch` — take it out of the inbox. This is also what Mute
    /// means on a row: the row's absence from the next push is the answer.
    pub(crate) fn tasks_unwatch(&mut self, params: &Value) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        let (project_id, task) = self.tracker_task(&task_id)?;
        self.set_watching(&project_id, task, false, Actor::User)
    }

    /// Start or stop the user watching, writing the event only when something
    /// actually changed.
    pub(in crate::app) fn set_watching(
        &mut self,
        project_id: &str,
        task: Task,
        watching: bool,
        actor: Actor,
    ) -> Result<Value, String> {
        let now = crate::store::now_rfc3339();
        let mut write = TaskWrite::by(actor.clone(), task);
        if !write.task.set_watched(watching) {
            return Ok(json!({
                "task": self.task_json_with_live_identities(project_id, &write.task),
            }));
        }
        let kind = if watching {
            TaskEventKind::Watched
        } else {
            TaskEventKind::Unwatched
        };
        write.event(&actor, kind, json!({}), &now);
        self.commit_task_write(project_id, write, &now)
    }

    /// `tasks.read_through` — the user has read this task as far as
    /// `event_id`.
    ///
    /// Advanced by the task page on open and on reaching the end, the way a
    /// conversation's read mark is. Never moved backwards: a reader who opens
    /// an old task after a newer one has still read the newer one, and a mark
    /// that walked back would make everything unread again.
    pub(crate) fn tasks_read_through(&mut self, params: &Value) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        let event_id = require_str(params, "event_id")?;
        let (project_id, task) = self.tracker_task(&task_id)?;
        let now = crate::store::now_rfc3339();
        let mut write = TaskWrite::by(Actor::User, task);
        let already_read =
            |read: &str| super::inbox::when(read) >= super::inbox::when(event_id.as_str());
        if write.task.read_through.as_deref().is_some_and(already_read) {
            return Ok(json!({
                "task": self.task_json_with_live_identities(&project_id, &write.task),
            }));
        }
        write.task.read_through = Some(event_id);
        // No event: reading is not something that happened TO the task, and a
        // timeline that recorded every scroll would be a timeline nobody could
        // read.
        self.commit_task_write(&project_id, write, &now)
    }

    /// `tasks.dismiss` — clear this task's inbox row until something else
    /// happens to it.
    ///
    /// The same Done a conversation row has. A mark rather than a flag: the
    /// next event is past it and the row comes back on its own, so nothing has
    /// to remember to unset anything.
    pub(crate) fn tasks_dismiss(&mut self, params: &Value) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        let (project_id, task) = self.tracker_task(&task_id)?;
        let newest = self
            .tracker_store()?
            .load_tracker_timeline(&task.id)
            .stored()?
            .last()
            .map(|entry| match entry {
                crate::tracker::TimelineEntry::Comment(comment) => comment.id.clone(),
                crate::tracker::TimelineEntry::Event(event) => event.id.clone(),
            });
        let now = crate::store::now_rfc3339();
        let mut write = TaskWrite::by(Actor::User, task);
        write.task.dismissed_through = newest;
        // No event: clearing a row is the reader tidying their own inbox, not
        // something that happened to the task.
        self.commit_task_write(&project_id, write, &now)
    }

    /// `tasks.for_agent` — what one agent holds and what it watches.
    ///
    /// Two digest lists rather than two whole-task lists: this is a list
    /// somebody scans, and the body of thirty tasks is not a list. A task
    /// the agent both holds and watches appears in both, because the two
    /// questions are different and a client showing one should not have to
    /// know about the other.
    pub(crate) fn tasks_for_agent(&mut self, params: &Value) -> Result<Value, String> {
        let agent_id = require_str(params, "agent_id")?;
        let entity_id = self
            .entity_of_agent(&agent_id)
            .ok_or_else(|| format!("unknown agent_id: {agent_id}"))?;
        let project_id = self
            .projects
            .project_id_of(&entity_id)
            .map(str::to_string)
            .ok_or_else(|| format!("agent {agent_id} belongs to no project"))?;
        let project_path = self.tracker_project_path(&project_id)?;
        let mut tasks = self
            .tracker_store()?
            .list_tracker_tasks(&project_path, TaskFilter::default())
            .stored()?;
        // Newest-updated first: what a reader wants off a list like this is
        // what moved, and `tasks.list`'s own order is by number, which is
        // when it was filed rather than when it last mattered.
        tasks.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
        let digest = |task: &Task| {
            json!({
                "task_id": task.id,
                "number": task.number,
                "title": task.title,
                "state": task.state.as_str(),
                "status": task.status,
                "updated_at": task.updated_at,
            })
        };
        let assigned: Vec<Value> = tasks
            .iter()
            .filter(|task| {
                task.assignee
                    .as_ref()
                    .and_then(crate::tracker::Assignee::agent_id)
                    == Some(agent_id.as_str())
            })
            .map(digest)
            .collect();
        let tracking: Vec<Value> = tasks
            .iter()
            .filter(|task| task.is_tracked_by(&agent_id))
            .map(digest)
            .collect();
        Ok(json!({
            "agent_id": agent_id,
            "assigned": assigned,
            "tracking": tracking,
        }))
    }
}
