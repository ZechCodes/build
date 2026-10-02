//! What happens to a task without anyone asking (spec: Tasks → Automatic
//! activity).
//!
//! Two things, and both exist because a task that does not keep up with the
//! work is worse than no task: it says something false about where the work
//! got to, and a board nobody trusts is a board nobody reads.
//!
//! 1. **An agent holding a dispatched task reports Complete.** The task
//!    moves to In review.
//! 2. **A workspace a task links is finished after a merge.** The task
//!    closes, and when Done deleted the branch too, its timeline says which.
//!    Finishing before merge leaves the task open.
//!
//! Neither moves the inbox anchor or crosses a dismissal line. They are the
//! work happening, not somebody speaking to the human.
//!
//! **What a report does NOT do is comment.** It used to: the report's body was
//! copied onto the task. That made every end-of-turn report a task comment,
//! including the four an agent wrote answering a reminder it could not
//! silence — five comments on #27 in three minutes, none of them written to
//! the task. A conversation message is not a comment on a task, whatever it
//! mentions; `comment_task` and `tasks.comment` are the two things that
//! write one, and an agent that wants its report on the task calls one of
//! them. The prompt asks it to.

use super::{edits, StoredAnswer, TaskWrite};
use crate::app::AppState;
use crate::store::TaskFilter;
use crate::tracker::{Actor, Task, TaskEventKind, TaskState, IN_REVIEW_STATUS};
use serde_json::json;

impl AppState {
    /// An agent reported Complete. Move the task THIS TURN was dispatched
    /// under, and nothing else.
    ///
    /// Complete means the work is ready to be looked at, which is what In
    /// review means on a board — so the card follows the report without
    /// anybody dragging it. A **Blocked or Failed** report moves nothing:
    /// blocked is not ready to look at, and a board that said it was would be
    /// lying in the direction that wastes a reviewer's time.
    ///
    /// The timeline is the record of this, and it names the agent: the `moved`
    /// event carries `by: "report"`, so a reader can tell a card the agent
    /// moved deliberately from one its report moved for it.
    ///
    /// Quiet about its own failure. The report is the agent's and the turn is
    /// over; failing to move a card must not turn a finished piece of work
    /// into a failed one.
    pub(in crate::app) fn move_held_task_on_complete(&mut self, entity_id: &str, agent_id: &str) {
        let Some((project_id, task)) = self.task_this_turn_was_for(entity_id, agent_id) else {
            return;
        };
        match self.task_has_review(&task.id) {
            Ok(true) => return,
            Ok(false) => {}
            Err(error) => {
                eprintln!("check review on task {} after report: {error}", task.id);
                return;
            }
        }
        if let Err(error) = self.hand_held_task_on(&project_id, task) {
            eprintln!("move the task {agent_id} holds on its report: {error}");
        }
    }

    fn hand_held_task_on(&mut self, project_id: &str, task: Task) -> Result<(), String> {
        if !task.is_open() {
            return Ok(());
        }
        let actor = Actor::Agent {
            agent_id: self
                .task_holder(&task)
                .unwrap_or_else(|| "agent".to_string()),
        };
        let now = crate::store::now_rfc3339();
        let mut write = TaskWrite::by(actor.clone(), task);
        edits::move_to(
            &mut write,
            IN_REVIEW_STATUS,
            &actor,
            json!({ "by": "report" }),
            &now,
        );
        if write.events.is_empty() {
            // Already there. Committing nothing would still push and still
            // notify every tracker that the task "changed".
            return Ok(());
        }
        self.commit_task_write(project_id, write, &now).map(|_| ())
    }

    /// The task THIS TURN was dispatched under, when an assignment started it.
    ///
    /// Not "a task this agent holds". An agent commonly holds a queue: it is
    /// assigned three tasks, works one, and reports. Picking the newest of
    /// the three moved a task nobody had touched — twice in one day — and
    /// the agent had to move it back by hand and warn the project agent not to
    /// roll it.
    ///
    /// So the answer is the one the dispatch recorded when it started the
    /// turn, and `None` for a turn nobody dispatched — a reviewer message, a
    /// notice, a restart. A report on a turn that was not about a task says
    /// nothing about any task.
    ///
    /// Taken rather than read: the marker belongs to one turn, and the report
    /// is the end of it. A second Complete in the same turn moves nothing,
    /// which is right — the card is already where the first one put it.
    fn task_this_turn_was_for(
        &mut self,
        entity_id: &str,
        agent_id: &str,
    ) -> Option<(String, Task)> {
        let task_id = self.dispatched_task.remove(agent_id)?;
        let project_id = self.projects.project_id_of(entity_id)?.to_string();
        let task = self
            .tracker_store()
            .ok()?
            .load_tracker_task(&task_id)
            .ok()??;
        // Still this agent's. A reassignment between the dispatch and the
        // report means somebody else holds it now, and their card is not this
        // report's to move.
        if task
            .assignee
            .as_ref()
            .and_then(crate::tracker::Assignee::agent_id)
            != Some(agent_id)
        {
            return None;
        }
        Some((project_id, task))
    }

    fn task_holder(&self, task: &Task) -> Option<String> {
        task.assignee
            .as_ref()
            .and_then(crate::tracker::Assignee::agent_id)
            .map(str::to_string)
    }

    /// A merged workspace is being finished: close every open task linking it.
    ///
    /// Called when Done is ACCEPTED rather than after the folder is gone,
    /// because the merged run state is already durable and a removal that
    /// later fails on disk does not make the work un-done.
    ///
    /// Quiet about its own failure, for the reason the report is: Done is the
    /// user's action and it succeeded.
    pub(in crate::app) fn close_tasks_of_finished_workspace(
        &mut self,
        project_id: &str,
        workspace_id: &str,
    ) {
        let Ok(project_path) = self.tracker_project_path(project_id) else {
            return;
        };
        let open = self.tracker_store().and_then(|store| {
            store
                .list_tracker_tasks(
                    &project_path,
                    TaskFilter {
                        state: Some(TaskState::Open),
                        status: None,
                    },
                )
                .stored()
        });
        let Ok(open) = open else {
            return;
        };
        let now = crate::store::now_rfc3339();
        for task in open
            .into_iter()
            .filter(|task| task.links.links_workspace(workspace_id))
        {
            match self.task_has_review(&task.id) {
                Ok(true) => continue,
                Ok(false) => {}
                Err(error) => {
                    eprintln!(
                        "check review on task {} during workspace finish: {error}",
                        task.id
                    );
                    continue;
                }
            }
            let mut write = TaskWrite::by(Actor::User, task);
            write.task.state = TaskState::Closed;
            write.task.closed_at = Some(now.clone());
            write.event(
                &Actor::User,
                TaskEventKind::Closed,
                json!({ "reason": "workspace_finished", "workspace_id": workspace_id }),
                &now,
            );
            if let Err(error) = self.commit_task_write(project_id, write, &now) {
                eprintln!("close task for finished workspace {workspace_id}: {error}");
            }
        }
    }

    fn task_has_review(&self, task_id: &str) -> Result<bool, String> {
        self.tracker_store()?
            .load_review(task_id)
            .map(|review| review.is_some())
            .map_err(|error| error.to_string())
    }

    /// Every task of the project that links the workspace or the branch,
    /// open or closed. None when the project's tasks cannot be read.
    pub(in crate::app) fn tasks_linking_workspace_or_branch(
        &self,
        project_id: &str,
        workspace_id: &str,
        branch: &str,
    ) -> Vec<Task> {
        let Ok(project_path) = self.tracker_project_path(project_id) else {
            return Vec::new();
        };
        self.tracker_store()
            .and_then(|store| {
                store
                    .list_tracker_tasks(&project_path, TaskFilter::default())
                    .stored()
            })
            .unwrap_or_default()
            .into_iter()
            .filter(|task| {
                task.links.links_workspace(workspace_id)
                    || task.links.branches.iter().any(|linked| linked == branch)
            })
            .collect()
    }

    /// Done deleted `branch` along with the workspace: say so on every task
    /// that links either, open or already closed by that same Done, so the
    /// timeline says where the branch went.
    ///
    /// Quiet about its own failure: the branch is gone either way.
    pub(in crate::app) fn note_branch_deleted(
        &mut self,
        project_id: &str,
        workspace_id: &str,
        branch: &str,
        reason: Option<&str>,
    ) {
        let now = crate::store::now_rfc3339();
        for task in self.tasks_linking_workspace_or_branch(project_id, workspace_id, branch) {
            let mut write = TaskWrite::by(Actor::User, task);
            write.event(
                &Actor::User,
                TaskEventKind::BranchDeleted,
                match reason {
                    Some(reason) => {
                        json!({ "branch": branch, "workspace_id": workspace_id, "reason": reason })
                    }
                    None => json!({ "branch": branch, "workspace_id": workspace_id }),
                },
                &now,
            );
            if let Err(error) = self.commit_task_write(project_id, write, &now) {
                eprintln!("note deleted branch {branch} on its task: {error}");
            }
        }
    }
}
