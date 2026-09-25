//! Durable names for agents an issue mentions. A Done workspace loses its
//! conversation, so these facts live on the issue before that happens.

use super::StoredAnswer;
use crate::app::AppState;
use crate::store::{PersistedPlan, PersistedRun};
use crate::tracker::{Actor, Issue, IssueAgentIdentity, TimelineEntry};
use std::cell::OnceCell;
use std::collections::{BTreeSet, HashMap};

/// The store's word on agents no live roster holds, read at most once however
/// many issues one call resolves identities for.
///
/// Telling who a departed agent was reads every run and every plan, each with
/// its agents' conversation tails. Done again for every agent every issue
/// names, that read held the app lock for seconds at a time under
/// `issues.list` (#128); looking each agent up by restoring every stored
/// roster — each a copy of those tails — held it for another 450 ms on a real
/// store (#131). So the records are read once, without their conversations,
/// into an index of who every stored agent was. Nothing a call does
/// between two issues changes those records — a backfill writes the issue,
/// never a run — so the index stands for the rest of the call. Build one per
/// call, never keep one.
#[derive(Default)]
pub(super) struct StoredRosters {
    known: OnceCell<Option<HashMap<String, IssueAgentIdentity>>>,
}

fn mentioned(issue: &Issue, timeline: &[TimelineEntry]) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    add_actor(&mut ids, &issue.created_by);
    collect_written_agent_ids(&issue.title, &mut ids);
    collect_written_agent_ids(&issue.body, &mut ids);
    if let Some(id) = issue
        .assignee
        .as_ref()
        .and_then(|assignee| assignee.agent_id())
    {
        ids.insert(id.to_string());
    }
    ids.extend(issue.trackers.iter().cloned());
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

/// Explicit `@agent:<id>` references in issue prose name agents even when
/// they never acted on that issue. Agent ids contain letters, digits, `-` and
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

fn retain_known_fields(
    prior: &IssueAgentIdentity,
    current: IssueAgentIdentity,
) -> IssueAgentIdentity {
    if current.available {
        return current;
    }
    IssueAgentIdentity {
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

/// Every agent the stored plans name, as the identity an issue shows.
fn plan_identities(plans: &[PersistedPlan]) -> impl Iterator<Item = IssueAgentIdentity> + '_ {
    plans.iter().flat_map(|plan| {
        plan.members().into_iter().map(|member| IssueAgentIdentity {
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
    /// Migrate every issue before a roster can disappear. An old author's
    /// only reference may be in a closed, unlinked issue's timeline; reads
    /// and the workspace's automatic close operation cannot cover that case.
    /// A failed save refuses removal while the source records still exist.
    pub(in crate::app) fn preserve_project_issue_identities(
        &self,
        project_id: &str,
    ) -> Result<(), String> {
        let Some(store) = &self.store else {
            return Ok(());
        };
        let project_path = self.tracker_project_path(project_id)?;
        let issues = store
            .list_tracker_issues(&project_path, crate::store::IssueFilter::default())
            .stored()?;
        let ids: Vec<String> = issues.iter().map(|issue| issue.id.clone()).collect();
        let mut timelines = store.load_tracker_timelines(&ids).stored()?;
        let rosters = StoredRosters::default();
        for issue in issues {
            let timeline = timelines.remove(&issue.id).unwrap_or_default();
            self.backfill_issue_identities(issue, &timeline, &rosters)?;
        }
        Ok(())
    }

    pub(in crate::app) fn preserve_entity_issue_identities(
        &self,
        entity_id: &str,
    ) -> Result<(), String> {
        if let Some(project_id) = self.projects.project_id_of(entity_id) {
            self.preserve_project_issue_identities(project_id)?;
        }
        Ok(())
    }

    /// Resolve the invalidation before changing the roster: a store failure
    /// must not remove an agent and then leave other clients' issue caches
    /// pointing at it. Preservation has already filled historical identities,
    /// including those on closed issues with no workspace link.
    pub(in crate::app) fn issues_with_agent_identity(
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
        let issue_ids: Vec<_> = store
            .list_tracker_issues(&project_path, crate::store::IssueFilter::default())
            .stored()?
            .into_iter()
            .filter(|issue| issue.identities.contains_key(agent_id))
            .map(|issue| issue.id)
            .collect();
        Ok((!issue_ids.is_empty()).then(|| (project_id.to_string(), issue_ids)))
    }

    pub(super) fn issue_json_with_live_identities(
        &self,
        project_id: &str,
        issue: &Issue,
    ) -> serde_json::Value {
        let issue = self.issue_with_read_identities(issue.clone(), &[], &StoredRosters::default());
        super::issue_json(project_id, &issue)
    }

    fn live_issue_identity(&self, agent_id: &str) -> Option<IssueAgentIdentity> {
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
        Some(IssueAgentIdentity {
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
    fn stored_issue_identity(
        &self,
        agent_id: &str,
        rosters: &StoredRosters,
    ) -> Option<IssueAgentIdentity> {
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
    fn stored_identities(&self) -> Option<HashMap<String, IssueAgentIdentity>> {
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
    ) -> impl Iterator<Item = IssueAgentIdentity> + 'a {
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
                .map(move |member| IssueAgentIdentity {
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

    fn resolved_issue_identity(
        &self,
        agent_id: &str,
        rosters: &StoredRosters,
    ) -> Option<IssueAgentIdentity> {
        self.live_issue_identity(agent_id)
            .or_else(|| self.stored_issue_identity(agent_id, rosters))
    }

    /// Add the identities this write can still inspect before a workspace is
    /// removed. Existing snapshots stay when an agent is no longer present.
    pub(super) fn capture_issue_identities(&self, issue: &mut Issue, timeline: &[TimelineEntry]) {
        self.capture_identities_with(issue, timeline, &StoredRosters::default());
    }

    fn capture_identities_with(
        &self,
        issue: &mut Issue,
        timeline: &[TimelineEntry],
        rosters: &StoredRosters,
    ) {
        for id in mentioned(issue, timeline) {
            if let Some(identity) = self.resolved_issue_identity(&id, rosters) {
                issue
                    .identities
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
    pub(super) fn backfill_issue_identities(
        &self,
        mut issue: Issue,
        timeline: &[TimelineEntry],
        rosters: &StoredRosters,
    ) -> Result<Issue, String> {
        let before = issue.identities.clone();
        self.capture_identities_with(&mut issue, timeline, rosters);
        if issue.identities != before {
            self.tracker_store()?
                .save_tracker_issue_activity(&issue, &[], &[])
                .stored()?;
        }
        Ok(issue)
    }

    /// Fill old issues from living agents and mark departed ones unavailable.
    /// Missing agents still get a row, so the client has an honest fallback.
    pub(super) fn issue_with_read_identities(
        &self,
        mut issue: Issue,
        timeline: &[TimelineEntry],
        rosters: &StoredRosters,
    ) -> Issue {
        let ids = mentioned(&issue, timeline);
        for id in ids {
            if let Some(identity) = self.resolved_issue_identity(&id, rosters) {
                issue
                    .identities
                    .entry(id)
                    .and_modify(|saved| {
                        *saved = retain_known_fields(saved, identity.clone());
                    })
                    .or_insert(identity);
            } else {
                issue
                    .identities
                    .entry(id.clone())
                    .or_insert_with(|| IssueAgentIdentity {
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
        for identity in issue.identities.values_mut() {
            identity.available = self
                .live_issue_identity(&identity.agent_id)
                .is_some_and(|live| live.available);
        }
        issue
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn assignment_history_keeps_agent_ids_after_reassignment() {
        let mut issue = Issue::drafted("/repo", "one", Actor::User, "2026-01-01T00:00:00Z");
        issue.assignee = Some(crate::tracker::Assignee::User);
        let assigned = crate::tracker::IssueEvent::new(
            &issue.id,
            Actor::User,
            crate::tracker::IssueEventKind::Assigned,
            json!({ "assignee": { "kind": "agent", "agent_id": "agent-before" } }),
            "2026-01-01T00:00:00Z",
        );
        let dispatched = crate::tracker::IssueEvent::new(
            &issue.id,
            Actor::User,
            crate::tracker::IssueEventKind::Dispatched,
            json!({ "agent_id": "agent-after" }),
            "2026-01-01T00:00:01Z",
        );
        let ids = mentioned(
            &issue,
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
        let mut issue = Issue::drafted("/repo", "one", Actor::User, "2026-01-01T00:00:00Z");
        issue.body = "Ask @agent:agent-4. Then check @agent:project-7".into();
        assert_eq!(
            mentioned(&issue, &[]),
            BTreeSet::from(["agent-4".to_string(), "project-7".to_string()])
        );
    }
}
