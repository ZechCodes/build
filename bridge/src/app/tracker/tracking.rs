//! Who is watching an issue, and what one agent is on (spec: Issues →
//! Tracking).
//!
//! Tracking is a set on the issue and two verbs over it. What tracking is FOR
//! — the notices a change delivers — is [`super::notices`]; this file is only
//! the membership and the per-agent read.

use super::{IssueWrite, StoredAnswer};
use crate::app::{require_str, AppState};
use crate::store::IssueFilter;
use crate::tracker::{Actor, Issue, IssueEventKind};
use serde_json::{json, Value};

impl AppState {
    /// `issues.track` — this agent wants to hear about this issue.
    ///
    /// Idempotent, and quiet about it: an agent already watching answers the
    /// issue unchanged with no event. A set does not record being told twice.
    pub(crate) fn issues_track(&mut self, params: &Value) -> Result<Value, String> {
        let (project_id, issue, agent_id) = self.tracking_request(params)?;
        self.set_tracking(&project_id, issue, &agent_id, true, Actor::User, None)
    }

    /// `issues.untrack` — stop hearing about it.
    pub(crate) fn issues_untrack(&mut self, params: &Value) -> Result<Value, String> {
        let (project_id, issue, agent_id) = self.tracking_request(params)?;
        self.set_tracking(&project_id, issue, &agent_id, false, Actor::User, None)
    }

    /// The issue and the agent a tracking call names, both checked against the
    /// project the issue belongs to.
    ///
    /// An agent of another project is refused by name, the way every
    /// project-scoped handler refuses one: an issue's watchers are its own
    /// project's agents, and an agent elsewhere could not read what it was
    /// told anyway.
    fn tracking_request(&mut self, params: &Value) -> Result<(String, Issue, String), String> {
        let issue_id = require_str(params, "issue_id")?;
        let agent_id = require_str(params, "agent_id")?;
        let (project_id, issue) = self.tracker_issue(&issue_id)?;
        let entity_id = self
            .entity_of_agent(&agent_id)
            .ok_or_else(|| format!("unknown agent_id: {agent_id}"))?;
        if self.projects.project_id_of(&entity_id) != Some(project_id.as_str()) {
            return Err(format!("agent {agent_id} is not in project {project_id}"));
        }
        Ok((project_id, issue, agent_id))
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
        issue: Issue,
        agent_id: &str,
        tracking: bool,
        actor: Actor,
        because: Option<&str>,
    ) -> Result<Value, String> {
        let now = crate::store::now_rfc3339();
        let mut write = IssueWrite::by(actor.clone(), issue);
        let changed = if tracking {
            write.issue.track(agent_id)?
        } else {
            write.issue.untrack(agent_id)
        };
        if !changed {
            // Nothing moved, so nothing is written and nothing is pushed: a
            // set does not record being told twice, and a timeline that said
            // otherwise would be claiming a second fact for one truth.
            return Ok(json!({
                "issue": super::issue_json(project_id, &write.issue),
            }));
        }
        let mut payload = json!({ "agent_id": agent_id });
        if let (Some(because), Some(payload)) = (because, payload.as_object_mut()) {
            payload.insert("by".to_string(), json!(because));
        }
        let kind = if tracking {
            IssueEventKind::Tracked
        } else {
            IssueEventKind::Untracked
        };
        write.event(&actor, kind, payload, &now);
        self.commit_issue_write(project_id, write, &now)
    }

    /// `issues.watch` — the user wants this issue in their inbox.
    ///
    /// Idempotent and quiet about it, like `issues.track`: watching a second
    /// time is not a second fact, and a timeline that said so would be
    /// claiming two.
    pub(crate) fn issues_watch(&mut self, params: &Value) -> Result<Value, String> {
        let issue_id = require_str(params, "issue_id")?;
        let (project_id, issue) = self.tracker_issue(&issue_id)?;
        self.set_watching(&project_id, issue, true, Actor::User)
    }

    /// `issues.unwatch` — take it out of the inbox. This is also what Mute
    /// means on a row: the row's absence from the next push is the answer.
    pub(crate) fn issues_unwatch(&mut self, params: &Value) -> Result<Value, String> {
        let issue_id = require_str(params, "issue_id")?;
        let (project_id, issue) = self.tracker_issue(&issue_id)?;
        self.set_watching(&project_id, issue, false, Actor::User)
    }

    /// Start or stop the user watching, writing the event only when something
    /// actually changed.
    pub(in crate::app) fn set_watching(
        &mut self,
        project_id: &str,
        issue: Issue,
        watching: bool,
        actor: Actor,
    ) -> Result<Value, String> {
        let now = crate::store::now_rfc3339();
        let mut write = IssueWrite::by(actor.clone(), issue);
        if !write.issue.set_watched(watching) {
            return Ok(json!({
                "issue": super::issue_json(project_id, &write.issue),
            }));
        }
        let kind = if watching {
            IssueEventKind::Watched
        } else {
            IssueEventKind::Unwatched
        };
        write.event(&actor, kind, json!({}), &now);
        self.commit_issue_write(project_id, write, &now)
    }

    /// `issues.read_through` — the user has read this issue as far as
    /// `event_id`.
    ///
    /// Advanced by the issue page on open and on reaching the end, the way a
    /// conversation's read mark is. Never moved backwards: a reader who opens
    /// an old issue after a newer one has still read the newer one, and a mark
    /// that walked back would make everything unread again.
    pub(crate) fn issues_read_through(&mut self, params: &Value) -> Result<Value, String> {
        let issue_id = require_str(params, "issue_id")?;
        let event_id = require_str(params, "event_id")?;
        let (project_id, issue) = self.tracker_issue(&issue_id)?;
        let now = crate::store::now_rfc3339();
        let mut write = IssueWrite::by(Actor::User, issue);
        let already_read =
            |read: &str| super::inbox::when(read) >= super::inbox::when(event_id.as_str());
        if write
            .issue
            .read_through
            .as_deref()
            .is_some_and(already_read)
        {
            return Ok(json!({
                "issue": super::issue_json(&project_id, &write.issue),
            }));
        }
        write.issue.read_through = Some(event_id);
        // No event: reading is not something that happened TO the issue, and a
        // timeline that recorded every scroll would be a timeline nobody could
        // read.
        self.commit_issue_write(&project_id, write, &now)
    }

    /// `issues.dismiss` — clear this issue's inbox row until something else
    /// happens to it.
    ///
    /// The same Done a conversation row has. A mark rather than a flag: the
    /// next event is past it and the row comes back on its own, so nothing has
    /// to remember to unset anything.
    pub(crate) fn issues_dismiss(&mut self, params: &Value) -> Result<Value, String> {
        let issue_id = require_str(params, "issue_id")?;
        let (project_id, issue) = self.tracker_issue(&issue_id)?;
        let newest = self
            .tracker_store()?
            .load_tracker_timeline(&issue.id)
            .stored()?
            .last()
            .map(|entry| match entry {
                crate::tracker::TimelineEntry::Comment(comment) => comment.id.clone(),
                crate::tracker::TimelineEntry::Event(event) => event.id.clone(),
            });
        let now = crate::store::now_rfc3339();
        let mut write = IssueWrite::by(Actor::User, issue);
        write.issue.dismissed_through = newest;
        // No event: clearing a row is the reader tidying their own inbox, not
        // something that happened to the issue.
        self.commit_issue_write(&project_id, write, &now)
    }

    /// `issues.for_agent` — what one agent holds and what it watches.
    ///
    /// Two digest lists rather than two whole-issue lists: this is a list
    /// somebody scans, and the body of thirty issues is not a list. An issue
    /// the agent both holds and watches appears in both, because the two
    /// questions are different and a client showing one should not have to
    /// know about the other.
    pub(crate) fn issues_for_agent(&mut self, params: &Value) -> Result<Value, String> {
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
        let mut issues = self
            .tracker_store()?
            .list_tracker_issues(&project_path, IssueFilter::default())
            .stored()?;
        // Newest-updated first: what a reader wants off a list like this is
        // what moved, and `issues.list`'s own order is by number, which is
        // when it was filed rather than when it last mattered.
        issues.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
        let digest = |issue: &Issue| {
            json!({
                "issue_id": issue.id,
                "number": issue.number,
                "title": issue.title,
                "state": issue.state.as_str(),
                "status": issue.status,
                "updated_at": issue.updated_at,
            })
        };
        let assigned: Vec<Value> = issues
            .iter()
            .filter(|issue| {
                issue
                    .assignee
                    .as_ref()
                    .and_then(crate::tracker::Assignee::agent_id)
                    == Some(agent_id.as_str())
            })
            .map(digest)
            .collect();
        let tracking: Vec<Value> = issues
            .iter()
            .filter(|issue| issue.is_tracked_by(&agent_id))
            .map(digest)
            .collect();
        Ok(json!({
            "agent_id": agent_id,
            "assigned": assigned,
            "tracking": tracking,
        }))
    }
}
