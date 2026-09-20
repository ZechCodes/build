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
        let mut write = IssueWrite::of(issue);
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
