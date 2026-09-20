//! What an agent may do to its project's issues (spec: Issues → The MCP
//! tools).
//!
//! Eight tools, on the coding and project surfaces alike, each a thin wrapper
//! over the verb of the same shape: the same code path, the same refusals, the
//! same record afterwards. What the wrapper adds is who is calling.
//!
//! Two things are never arguments. **The project** comes from the calling
//! agent's conversation owner binding, so a tool call cannot reach a project
//! this agent does not work on however it is spelled — a call carrying a
//! project id is parsed as though it had not. **The author** is the calling
//! agent, so a comment is signed by whoever wrote it and an event by whoever
//! caused it, rather than by whoever the call claims.

use super::{edits, AssignTarget, IssueWrite, StoredAnswer};
use crate::app::AppState;
use crate::mcp::BridgeAction;
use crate::tracker::{Actor, MAX_BODY_BYTES};
use serde_json::{json, Value};

impl AppState {
    /// The tracker's tools. `None` is "not one of mine", which is every other
    /// action either working surface answers.
    pub(in crate::app) fn issue_surface_action(
        &mut self,
        entity_id: &str,
        agent_id: &str,
        action: &BridgeAction,
    ) -> Option<Result<Value, String>> {
        // Resolved once, before any arm runs: an agent whose owner is bound to
        // no project has no issues to reach, and saying so once beats each arm
        // discovering it.
        let scope = match self.issue_scope(entity_id, agent_id) {
            Ok(scope) => scope,
            Err(refusal) => return is_an_issue_tool(action).then_some(Err(refusal)),
        };
        Some(match action {
            BridgeAction::TrackerListIssues {
                state,
                status,
                label,
            } => self.issues_list(&scope.params(asked(&[
                ("state", state),
                ("status", status),
                ("label", label),
            ]))),
            BridgeAction::TrackerGetIssue { issue_id } => {
                self.scoped_issue_call(&scope, issue_id, json!({}), AppState::issues_get)
            }
            BridgeAction::TrackerCreateIssue {
                title,
                body,
                status,
                labels,
                priority,
            } => self.create_issue_as_agent(&scope, title, body, status, labels, priority),
            BridgeAction::TrackerCommentIssue {
                issue_id,
                body,
                refs,
            } => self.comment_issue_as_agent(&scope, issue_id, body, refs),
            BridgeAction::TrackerAssignIssue {
                issue_id,
                assignee,
                note,
            } => self.assign_issue_as_agent(&scope, issue_id, assignee, note.clone()),
            BridgeAction::TrackerMoveIssue { issue_id, status } => {
                self.move_issue_as_agent(&scope, issue_id, status)
            }
            BridgeAction::TrackerCloseIssue { issue_id, reason } => {
                self.close_issue_as_agent(&scope, issue_id, reason.clone())
            }
            BridgeAction::TrackerLinkIssue {
                issue_id,
                workspace_id,
                branch,
                commit,
                conversation_id,
                parent_issue_id,
            } => self.link_issue_as_agent(
                &scope,
                issue_id,
                asked(&[
                    ("workspace_id", workspace_id),
                    ("branch", branch),
                    ("commit", commit),
                    ("conversation_id", conversation_id),
                    ("parent_issue_id", parent_issue_id),
                ]),
            ),
            _ => return None,
        })
    }

    /// Which project's issues this agent reaches, and whose name goes on what
    /// it does.
    fn issue_scope(&self, entity_id: &str, agent_id: &str) -> Result<IssueScope, String> {
        let project_id = self
            .projects
            .project_id_of(entity_id)
            .map(str::to_string)
            .ok_or_else(|| format!("{entity_id} belongs to no project"))?;
        Ok(IssueScope {
            project_id,
            entity_id: entity_id.to_string(),
            actor: Actor::Agent {
                agent_id: agent_id.to_string(),
            },
        })
    }

    /// A read or write about one issue, refused unless that issue is this
    /// agent's project's.
    ///
    /// An issue of another project reads as unknown rather than as forbidden:
    /// the agent cannot list it, cannot have been handed it, and telling it
    /// that an id it guessed exists somewhere else says more than it asked.
    fn scoped_issue_call(
        &mut self,
        scope: &IssueScope,
        issue_id: &str,
        mut params: Value,
        call: fn(&mut AppState, &Value) -> Result<Value, String>,
    ) -> Result<Value, String> {
        self.issue_of_this_agents_project(scope, issue_id)?;
        params["issue_id"] = json!(issue_id);
        call(self, &params)
    }

    /// The issue, when it is one this agent may act on.
    fn issue_of_this_agents_project(
        &mut self,
        scope: &IssueScope,
        issue_id: &str,
    ) -> Result<crate::tracker::Issue, String> {
        let (project_id, issue) = self.tracker_issue(issue_id)?;
        if project_id != scope.project_id {
            return Err(format!("unknown issue_id: {issue_id}"));
        }
        Ok(issue)
    }

    fn create_issue_as_agent(
        &mut self,
        scope: &IssueScope,
        title: &str,
        body: &Option<String>,
        status: &Option<String>,
        labels: &[String],
        priority: &Option<String>,
    ) -> Result<Value, String> {
        let project_path = self.tracker_project_path(&scope.project_id)?;
        let now = crate::store::now_rfc3339();
        let mut params = asked(&[("body", body), ("status", status), ("priority", priority)]);
        params["title"] = json!(title);
        params["labels"] = json!(labels);
        let draft = edits::drafted_issue(&params, &project_path, scope.actor.clone(), &now)?;
        let created = crate::tracker::IssueEvent::new(
            &draft.id,
            scope.actor.clone(),
            crate::tracker::IssueEventKind::Created,
            json!({ "title": draft.title }),
            &now,
        );
        let issue = self
            .tracker_store()?
            .create_tracker_issue(draft, &[created])
            .stored()?;
        Ok(json!({
            "issue": super::issue_json(&scope.project_id, &issue),
        }))
    }

    fn comment_issue_as_agent(
        &mut self,
        scope: &IssueScope,
        issue_id: &str,
        body: &str,
        refs: &[crate::thread::ThreadLink],
    ) -> Result<Value, String> {
        let issue = self.issue_of_this_agents_project(scope, issue_id)?;
        let params = json!({ "body": body, "refs": refs });
        let body = edits::required_text(&params, "body", MAX_BODY_BYTES)?;
        let refs = super::refs::fenced_refs(&params, &issue, &self.issue_checkout_ids(&issue))?;
        let now = crate::store::now_rfc3339();
        let comment = crate::tracker::IssueComment {
            id: crate::tracker::new_comment_id(),
            issue_id: issue.id.clone(),
            author: scope.actor.clone(),
            body,
            refs,
            created_at: now.clone(),
        };
        let mut write = IssueWrite::of(issue);
        write.comments.push(comment.clone());
        let answered = self.commit_issue_write(&scope.project_id, write, &now)?;
        Ok(json!({
            "issue": answered["issue"],
            "comment": serde_json::to_value(&comment).map_err(|error| error.to_string())?,
        }))
    }

    fn assign_issue_as_agent(
        &mut self,
        scope: &IssueScope,
        issue_id: &str,
        assignee: &Value,
        note: Option<String>,
    ) -> Result<Value, String> {
        let issue = self.issue_of_this_agents_project(scope, issue_id)?;
        // A TOOL spells the agent's choice `harness`; the daemon's parse reads
        // `provider`, which is what the wire and `agent.add` call it. Mapped
        // here, at the one boundary where the two words meet.
        let target = AssignTarget::parse(Some(&provider_for_harness(assignee)))?;
        let sender = crate::app::AgentSender {
            entity_id: &scope.entity_id,
            agent_id: scope.actor.agent_id().unwrap_or_default(),
        };
        self.assign_issue_to(
            &scope.project_id,
            issue,
            target,
            note,
            scope.actor.clone(),
            Some(sender),
        )
    }

    fn move_issue_as_agent(
        &mut self,
        scope: &IssueScope,
        issue_id: &str,
        status: &str,
    ) -> Result<Value, String> {
        let issue = self.issue_of_this_agents_project(scope, issue_id)?;
        let params = json!({ "status": status });
        let status = edits::optional_status(&params, "status")?
            .ok_or_else(|| "status is required".to_string())?;
        let now = crate::store::now_rfc3339();
        let mut write = IssueWrite::of(issue);
        edits::move_to(&mut write, &status, &scope.actor, json!({}), &now);
        self.commit_issue_write(&scope.project_id, write, &now)
    }

    fn close_issue_as_agent(
        &mut self,
        scope: &IssueScope,
        issue_id: &str,
        reason: Option<String>,
    ) -> Result<Value, String> {
        let issue = self.issue_of_this_agents_project(scope, issue_id)?;
        if !issue.is_open() {
            return Err(format!("issue #{} is already closed", issue.number));
        }
        let now = crate::store::now_rfc3339();
        let mut write = IssueWrite::of(issue);
        edits::close(&mut write, &scope.actor, reason, &now);
        self.commit_issue_write(&scope.project_id, write, &now)
    }

    fn link_issue_as_agent(
        &mut self,
        scope: &IssueScope,
        issue_id: &str,
        params: Value,
    ) -> Result<Value, String> {
        let issue = self.issue_of_this_agents_project(scope, issue_id)?;
        let asked = edits::asked_links(&params)?;
        let now = crate::store::now_rfc3339();
        let mut write = IssueWrite::of(issue);
        self.apply_links(&scope.project_id, &mut write, &asked, &scope.actor, &now)?;
        self.commit_issue_write(&scope.project_id, write, &now)
    }
}

/// The params a tool asked for: the keys it named, and no key at all for the
/// ones it did not.
///
/// An absent argument is absent, never `null`. The verbs underneath read the
/// PRESENCE of a key to tell "narrow by this" from "do not narrow", and a
/// `null` is a value that says neither — `json!` would write one for every
/// `None`, which is how a tool that filtered nothing would be refused for
/// sending a status that is not a string.
fn asked(fields: &[(&str, &Option<String>)]) -> Value {
    let mut params = json!({});
    for (key, value) in fields {
        if let Some(value) = value {
            params[*key] = json!(value);
        }
    }
    params
}

/// Whether this action is one of the tracker's eight.
///
/// Asked only where the scope could not be resolved at all: an agent whose
/// owner is bound to no project has no issues to reach, and must still be able
/// to call every tool that is not about a project.
fn is_an_issue_tool(action: &BridgeAction) -> bool {
    matches!(
        action,
        BridgeAction::TrackerListIssues { .. }
            | BridgeAction::TrackerGetIssue { .. }
            | BridgeAction::TrackerCreateIssue { .. }
            | BridgeAction::TrackerCommentIssue { .. }
            | BridgeAction::TrackerAssignIssue { .. }
            | BridgeAction::TrackerMoveIssue { .. }
            | BridgeAction::TrackerCloseIssue { .. }
            | BridgeAction::TrackerLinkIssue { .. }
    )
}

/// Who is calling, and what that lets them reach.
struct IssueScope {
    project_id: String,
    /// The calling agent's conversation owner, for a delivery it asks for.
    entity_id: String,
    actor: Actor,
}

impl IssueScope {
    /// The agent's own project, written onto params it did not carry one in.
    fn params(&self, mut params: Value) -> Value {
        params["project_id"] = json!(self.project_id);
        params
    }
}

/// A tool's `harness` as the daemon's `provider`.
///
/// The one place the two words meet. Everything above this speaks the tool's
/// word and everything below it speaks the wire's, so neither surface has to
/// know about the other's spelling.
fn provider_for_harness(assignee: &Value) -> Value {
    let mut assignee = assignee.clone();
    let Some(object) = assignee.as_object_mut() else {
        return assignee;
    };
    if let Some(harness) = object.remove("harness") {
        object.entry("provider").or_insert(harness);
    }
    assignee
}
