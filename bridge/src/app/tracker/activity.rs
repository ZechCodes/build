//! What happens to an issue without anyone asking (spec: Issues → Automatic
//! activity).
//!
//! Two things, and both exist because an issue that does not keep up with the
//! work is worse than no issue: it says something false about where the work
//! got to, and a board nobody trusts is a board nobody reads.
//!
//! 1. **An agent holding a dispatched issue reports Complete.** The issue
//!    moves to In review.
//! 2. **A workspace an issue links is finished.** The issue closes.
//!
//! Neither moves the inbox anchor or crosses a dismissal line. They are the
//! work happening, not somebody speaking to the human.
//!
//! **What a report does NOT do is comment.** It used to: the report's body was
//! copied onto the issue. That made every end-of-turn report an issue comment,
//! including the four an agent wrote answering a reminder it could not
//! silence — five comments on #27 in three minutes, none of them written to
//! the issue. A conversation message is not a comment on an issue, whatever it
//! mentions; `comment_issue` and `issues.comment` are the two things that
//! write one, and an agent that wants its report on the issue calls one of
//! them. The prompt asks it to.

use super::{edits, IssueWrite, StoredAnswer};
use crate::app::AppState;
use crate::store::IssueFilter;
use crate::tracker::{Actor, Issue, IssueEventKind, IssueState, IN_REVIEW_STATUS};
use serde_json::json;

impl AppState {
    /// An agent reported Complete. If it holds a dispatched issue, move that
    /// issue to In review.
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
    pub(in crate::app) fn move_held_issue_on_complete(&mut self, entity_id: &str, agent_id: &str) {
        let Some((project_id, issue)) = self.issue_held_by(entity_id, agent_id) else {
            return;
        };
        if let Err(error) = self.hand_held_issue_on(&project_id, issue) {
            eprintln!("move the issue {agent_id} holds on its report: {error}");
        }
    }

    fn hand_held_issue_on(&mut self, project_id: &str, issue: Issue) -> Result<(), String> {
        if !issue.is_open() {
            return Ok(());
        }
        let actor = Actor::Agent {
            agent_id: self
                .issue_holder(&issue)
                .unwrap_or_else(|| "agent".to_string()),
        };
        let now = crate::store::now_rfc3339();
        let mut write = IssueWrite::by(actor.clone(), issue);
        edits::move_to(
            &mut write,
            IN_REVIEW_STATUS,
            &actor,
            json!({ "by": "report" }),
            &now,
        );
        if write.events.is_empty() {
            // Already there. Committing nothing would still push and still
            // notify every tracker that the issue "changed".
            return Ok(());
        }
        self.commit_issue_write(project_id, write, &now).map(|_| ())
    }

    /// The issue this agent is holding, when it is holding one.
    ///
    /// An agent holds an issue when the issue names it as assignee. Falling
    /// back to "the issue links this agent's conversation" would be wrong the
    /// moment two issues were ever dispatched into one conversation, which is
    /// ordinary — an agent that finished one issue and was handed another
    /// links both. The assignee is the one answer that stays true, and a
    /// dispatch is what sets it.
    ///
    /// The newest such issue, by number, so an agent that somehow holds two
    /// reports on the one it was given most recently rather than on all of
    /// them.
    fn issue_held_by(&mut self, entity_id: &str, agent_id: &str) -> Option<(String, Issue)> {
        let project_id = self.projects.project_id_of(entity_id)?.to_string();
        let project_path = self.tracker_project_path(&project_id).ok()?;
        let held = self
            .tracker_store()
            .ok()?
            .list_tracker_issues(
                &project_path,
                IssueFilter {
                    state: Some(IssueState::Open),
                    status: None,
                },
            )
            .ok()?
            .into_iter()
            .find(|issue| {
                issue
                    .assignee
                    .as_ref()
                    .and_then(crate::tracker::Assignee::agent_id)
                    == Some(agent_id)
            })?;
        Some((project_id, held))
    }

    fn issue_holder(&self, issue: &Issue) -> Option<String> {
        issue
            .assignee
            .as_ref()
            .and_then(crate::tracker::Assignee::agent_id)
            .map(str::to_string)
    }

    /// A workspace is being finished: close every open issue that links it.
    ///
    /// Called when Done is ACCEPTED rather than after the folder is gone,
    /// because eligibility is what proves the work is somewhere else — every
    /// commit already in the remote it pushes to — and a removal that later
    /// fails on disk does not make the work un-done.
    ///
    /// Quiet about its own failure, for the reason the report is: Done is the
    /// user's action and it succeeded.
    pub(in crate::app) fn close_issues_of_finished_workspace(
        &mut self,
        project_id: &str,
        workspace_id: &str,
    ) {
        let Ok(project_path) = self.tracker_project_path(project_id) else {
            return;
        };
        let open = self.tracker_store().and_then(|store| {
            store
                .list_tracker_issues(
                    &project_path,
                    IssueFilter {
                        state: Some(IssueState::Open),
                        status: None,
                    },
                )
                .stored()
        });
        let Ok(open) = open else {
            return;
        };
        let now = crate::store::now_rfc3339();
        for issue in open
            .into_iter()
            .filter(|issue| issue.links.links_workspace(workspace_id))
        {
            let mut write = IssueWrite::by(Actor::User, issue);
            write.issue.state = IssueState::Closed;
            write.issue.closed_at = Some(now.clone());
            write.event(
                &Actor::User,
                IssueEventKind::Closed,
                json!({ "reason": "workspace_finished", "workspace_id": workspace_id }),
                &now,
            );
            if let Err(error) = self.commit_issue_write(project_id, write, &now) {
                eprintln!("close issue for finished workspace {workspace_id}: {error}");
            }
        }
    }
}
