//! Who is watching a task, and what one agent is on (spec: Tasks →
//! Tracking).
//!
//! Tracking is a set on the task and two verbs over it. What tracking is FOR
//! — the notices a change delivers — is [`super::notices`]; this file is only
//! the membership and the per-agent read.

use super::TaskWrite;
use crate::app::{require_str, AppState};
use crate::tracker::{Actor, Task, TaskEventKind};
use serde_json::{json, Value};

impl AppState {
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
}
