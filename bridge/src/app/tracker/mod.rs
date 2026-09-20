//! The per-project issue tracker's verbs (spec: Issues).
//!
//! NOT `app/issues/`, which is the retired plan-and-stages flow. The two share
//! the English word and nothing else: different tables, different verbs, and
//! neither reads the other.
//!
//! Every verb here is the same three steps — resolve the issue (or the
//! project), decide what changed and what the timeline should say about it,
//! then write the record and its events in one store call and tell the project
//! something moved. The deciding is [`edits`]; the shape a client reads is
//! [`views`]; the fencing an agent-supplied reference goes through is
//! [`refs`].

mod activity;
mod dispatch;
mod edits;
mod notices;
mod refs;
mod tools;
mod tracking;
mod views;

pub(in crate::app) use dispatch::AssignTarget;
pub(in crate::app) use views::{columns_json, issue_json, issue_with_timeline_json};

use crate::app::{require_str, AppState};
use crate::store::{IssueFilter, Store};
use crate::tracker::{
    Actor, Issue, IssueComment, IssueEvent, IssueEventKind, IssueState, MAX_BODY_BYTES,
};
use serde_json::{json, Value};

/// What a verb is about to write: the record as it now stands, and everything
/// the timeline should say about how it got there.
///
/// Carried as one value because the two are one write — a refusal must not
/// leave a timeline claiming a move the record does not show — and because
/// every verb builds the same shape.
pub(in crate::app) struct IssueWrite {
    pub(in crate::app) issue: Issue,
    pub(in crate::app) comments: Vec<IssueComment>,
    pub(in crate::app) events: Vec<IssueEvent>,
    /// Who is making this change.
    ///
    /// Carried on the write rather than passed beside it: a tracking notice
    /// must not go back to whoever caused it, and that exclusion is the rule
    /// the whole feature rests on. An argument every caller had to remember to
    /// keep in step would be one a caller could get wrong.
    pub(in crate::app) actor: Actor,
}

impl IssueWrite {
    /// A write, and who is making it.
    fn by(actor: Actor, issue: Issue) -> IssueWrite {
        IssueWrite {
            issue,
            comments: Vec::new(),
            events: Vec::new(),
            actor,
        }
    }

    pub(in crate::app) fn event(
        &mut self,
        actor: &Actor,
        kind: IssueEventKind,
        payload: Value,
        now: &str,
    ) {
        self.events.push(IssueEvent::new(
            &self.issue.id,
            actor.clone(),
            kind,
            payload,
            now,
        ));
    }
}

impl AppState {
    // ------------------------------------------------------------- reads ---

    /// `issues.list` — one project's issues, newest first.
    ///
    /// `state` and `status` narrow the store read; `assignee` and `label` are
    /// applied to what it answers, because both live inside the record and
    /// hoisting a label list would mean a join table phase 1 does not need.
    pub(crate) fn issues_list(&mut self, params: &Value) -> Result<Value, String> {
        let project_id = require_str(params, "project_id")?;
        let project_path = self.tracker_project_path(&project_id)?;
        let state = edits::optional_state(params)?;
        let status = edits::optional_status(params, "status")?;
        let issues = self
            .tracker_store()?
            .list_tracker_issues(
                &project_path,
                IssueFilter {
                    state,
                    status: status.as_deref(),
                },
            )
            .stored()?;
        let assignee = edits::optional_assignee_filter(params)?;
        let label = crate::app::optional_nonempty_string(params, "label")?.map(str::to_string);
        let issues: Vec<Value> = issues
            .into_iter()
            .filter(|issue| assignee.matches(issue))
            .filter(|issue| edits::carries_label(issue, label.as_deref()))
            .map(|issue| issue_json(&project_id, &issue))
            .collect();
        Ok(json!({ "project_id": project_id, "issues": issues }))
    }

    /// `issues.get` — one issue and its whole timeline.
    pub(crate) fn issues_get(&mut self, params: &Value) -> Result<Value, String> {
        let issue_id = require_str(params, "issue_id")?;
        let (project_id, issue) = self.tracker_issue(&issue_id)?;
        let timeline = self
            .tracker_store()?
            .load_tracker_timeline(&issue.id)
            .stored()?;
        Ok(issue_with_timeline_json(&project_id, &issue, &timeline))
    }

    /// `issues.columns` — the board's columns, in board order.
    ///
    /// Takes a project it does not read, so the verb does not have to change
    /// when columns become per-project. A project that is not registered is
    /// still refused: answering for one is saying it exists.
    pub(crate) fn issues_columns(&mut self, params: &Value) -> Result<Value, String> {
        let project_id = require_str(params, "project_id")?;
        self.tracker_project_path(&project_id)?;
        Ok(columns_json(&project_id))
    }

    // ------------------------------------------------------------ writes ---

    /// `issues.create` — file one, mint its number, say it was created, and
    /// hand it over when it was filed with an assignee.
    ///
    /// Filing and assigning are one call because they are one thought: most
    /// issues an agent files are for somebody, and making the client do two
    /// round trips would leave an issue assigned to nobody in between for every
    /// failure of the second. The assignment is the WHOLE of `issues.assign` —
    /// the same delivery, the same events, the same deferral when it cuts a
    /// checkout — so there is one answer to what assigning means.
    pub(crate) fn issues_create(&mut self, params: &Value) -> Result<Value, String> {
        let project_id = require_str(params, "project_id")?;
        let project_path = self.tracker_project_path(&project_id)?;
        let actor = Actor::User;
        let now = crate::store::now_rfc3339();
        let draft = edits::drafted_issue(params, &project_path, actor.clone(), &now)?;
        // Read BEFORE the issue is written: an assignee this bridge cannot make
        // sense of must refuse the whole call rather than leave a filed issue
        // nobody asked for.
        let target = match params.get("assignee") {
            None | Some(Value::Null) => None,
            Some(assignee) => Some(AssignTarget::parse(Some(assignee))?),
        };
        let created = IssueEvent::new(
            &draft.id,
            actor.clone(),
            IssueEventKind::Created,
            json!({ "title": draft.title }),
            &now,
        );
        let issue = self
            .tracker_store()?
            .create_tracker_issue(draft, &[created])
            .stored()?;
        self.note_issues_changed(&project_id, &issue.id);
        let Some(target) = target else {
            return Ok(json!({
                "issue": issue_json(&project_id, &issue),
                "dispatch": Value::Null,
            }));
        };
        let note = crate::app::optional_nonempty_string(params, "note")?.map(str::to_string);
        self.assign_issue_to(&project_id, issue, target, note, actor, None)
    }

    /// `issues.update` — title, body, labels, priority, status, state.
    ///
    /// Only the fields present are applied, and each one that actually changes
    /// something writes its own event. A title, a body or a priority writes
    /// none: `updated_at` is the whole history those need.
    pub(crate) fn issues_update(&mut self, params: &Value) -> Result<Value, String> {
        let issue_id = require_str(params, "issue_id")?;
        let (project_id, issue) = self.tracker_issue(&issue_id)?;
        let now = crate::store::now_rfc3339();
        let mut write = IssueWrite::by(Actor::User, issue);
        edits::apply_update(&mut write, params, &Actor::User, &now)?;
        self.commit_issue_write(&project_id, write, &now)
    }

    /// `issues.comment` — say something, with typed references fenced twice.
    pub(crate) fn issues_comment(&mut self, params: &Value) -> Result<Value, String> {
        let issue_id = require_str(params, "issue_id")?;
        let (project_id, issue) = self.tracker_issue(&issue_id)?;
        let body = edits::required_text(params, "body", MAX_BODY_BYTES)?;
        let refs = refs::fenced_refs(params, &issue, &self.issue_checkout_ids(&issue))?;
        let now = crate::store::now_rfc3339();
        let comment = IssueComment {
            id: crate::tracker::new_comment_id(),
            issue_id: issue.id.clone(),
            author: Actor::User,
            body,
            refs,
            created_at: now.clone(),
        };
        let mut write = IssueWrite::by(Actor::User, issue);
        write.comments.push(comment.clone());
        let answered = self.commit_issue_write(&project_id, write, &now)?;
        Ok(json!({
            "issue": answered["issue"],
            "comment": serde_json::to_value(&comment).map_err(|error| error.to_string())?,
        }))
    }

    /// `issues.link` — one or more of the five link keys, each writing a
    /// `linked` event for the link that was not already there.
    pub(crate) fn issues_link(&mut self, params: &Value) -> Result<Value, String> {
        let issue_id = require_str(params, "issue_id")?;
        let (project_id, issue) = self.tracker_issue(&issue_id)?;
        let asked = edits::asked_links(params)?;
        let mut write = IssueWrite::by(Actor::User, issue);
        let now = crate::store::now_rfc3339();
        self.apply_links(&project_id, &mut write, &asked, &Actor::User, &now)?;
        self.commit_issue_write(&project_id, write, &now)
    }

    /// `issues.close` — closing an already closed issue is a conflict, not a
    /// silent no-op: the caller believed something that was not true.
    pub(crate) fn issues_close(&mut self, params: &Value) -> Result<Value, String> {
        let issue_id = require_str(params, "issue_id")?;
        let (project_id, issue) = self.tracker_issue(&issue_id)?;
        if !issue.is_open() {
            return Err(format!("issue #{} is already closed", issue.number));
        }
        let reason = crate::app::optional_nonempty_string(params, "reason")?.map(str::to_string);
        let now = crate::store::now_rfc3339();
        let mut write = IssueWrite::by(Actor::User, issue);
        edits::close(&mut write, &Actor::User, reason, &now);
        self.commit_issue_write(&project_id, write, &now)
    }

    /// `issues.reopen` — the same rule the other way.
    pub(crate) fn issues_reopen(&mut self, params: &Value) -> Result<Value, String> {
        let issue_id = require_str(params, "issue_id")?;
        let (project_id, issue) = self.tracker_issue(&issue_id)?;
        if issue.is_open() {
            return Err(format!("issue #{} is already open", issue.number));
        }
        let now = crate::store::now_rfc3339();
        let mut write = IssueWrite::by(Actor::User, issue);
        write.issue.state = IssueState::Open;
        write.issue.closed_at = None;
        write.event(&Actor::User, IssueEventKind::Reopened, json!({}), &now);
        self.commit_issue_write(&project_id, write, &now)
    }

    // ------------------------------------------------------------ shared ---

    /// Write a record and everything its timeline says about the change, then
    /// tell the project it moved.
    ///
    /// The one place a tracker write lands, so `updated_at`, the store call and
    /// the push note cannot be done three different ways by ten verbs.
    pub(in crate::app) fn commit_issue_write(
        &mut self,
        project_id: &str,
        mut write: IssueWrite,
        now: &str,
    ) -> Result<Value, String> {
        write.issue.updated_at = now.to_string();
        self.tracker_store()?
            .save_tracker_issue_activity(&write.issue, &write.comments, &write.events)
            .stored()?;
        self.note_issues_changed(project_id, &write.issue.id);
        // AFTER the write is durable, and quiet about its own failure: the
        // change landed, and a conversation that could not be written must not
        // turn it back into a refusal.
        self.notify_trackers(&write);
        Ok(json!({ "issue": issue_json(project_id, &write.issue) }))
    }

    /// Add the links asked for, each checked against the issue's own project
    /// and each writing one `linked` event.
    pub(in crate::app) fn apply_links(
        &mut self,
        project_id: &str,
        write: &mut IssueWrite,
        asked: &edits::AskedLinks,
        actor: &Actor,
        now: &str,
    ) -> Result<(), String> {
        for (kind, value) in asked.entries() {
            self.refuse_foreign_link(project_id, kind, value)?;
            let added = match kind {
                "workspace_id" => {
                    crate::tracker::IssueLinks::add(&mut write.issue.links.workspace_ids, value)
                }
                "branch" => crate::tracker::IssueLinks::add(&mut write.issue.links.branches, value),
                "commit" => crate::tracker::IssueLinks::add(&mut write.issue.links.commits, value),
                "conversation_id" => {
                    crate::tracker::IssueLinks::add(&mut write.issue.links.conversation_ids, value)
                }
                _ => self.link_parent(write, value)?,
            };
            if added {
                write.event(actor, IssueEventKind::Linked, json!({ kind: value }), now);
            }
        }
        Ok(())
    }

    /// A parent is one issue of the same project, never the issue itself and
    /// never a link that closes a loop: a cycle makes "the issues under this
    /// one" a question with no answer.
    fn link_parent(&mut self, write: &mut IssueWrite, parent_id: &str) -> Result<bool, String> {
        if parent_id == write.issue.id {
            return Err("an issue cannot be its own parent".to_string());
        }
        if write.issue.links.parent_issue_id.as_deref() == Some(parent_id) {
            return Ok(false);
        }
        self.refuse_parent_cycle(&write.issue, parent_id)?;
        write.issue.links.parent_issue_id = Some(parent_id.to_string());
        Ok(true)
    }

    /// Walk up from the proposed parent: reaching this issue would close a
    /// loop. Bounded by the chain it walks, which a refusal keeps acyclic.
    fn refuse_parent_cycle(&mut self, issue: &Issue, parent_id: &str) -> Result<(), String> {
        let mut at = Some(parent_id.to_string());
        let mut seen = 0usize;
        while let Some(id) = at {
            if id == issue.id {
                return Err(format!(
                    "issue {parent_id} is already below this one — a parent link cannot close a loop"
                ));
            }
            let Some(next) = self.tracker_store()?.load_tracker_issue(&id).stored()? else {
                return Err(format!("unknown parent_issue_id: {parent_id}"));
            };
            if next.project_path != issue.project_path {
                return Err(format!("issue {parent_id} is not in this issue's project"));
            }
            seen += 1;
            if seen > crate::tracker::MAX_LINKS_PER_KIND {
                return Err("parent chain is too deep".to_string());
            }
            at = next.links.parent_issue_id;
        }
        Ok(())
    }

    /// Refuse a link that names something of another project. A link is what
    /// an issue is about, and an issue is about its own project.
    fn refuse_foreign_link(&self, project_id: &str, kind: &str, value: &str) -> Result<(), String> {
        match kind {
            "workspace_id" => {
                let workspace = self
                    .workspaces
                    .get(value)
                    .ok_or_else(|| format!("unknown workspace_id: {value}"))?;
                if workspace.project_id != project_id {
                    return Err(format!("workspace {value} is not in project {project_id}"));
                }
            }
            "conversation_id" => {
                if self.projects.project_id_of(value) != Some(project_id) {
                    return Err(format!(
                        "conversation {value} is not in project {project_id}"
                    ));
                }
            }
            // A branch and a commit are words about a repository Build may not
            // have fetched yet. Shape is all there is to check — a commit is
            // held to being a sha — and the checkout is what settles the rest.
            "commit" if !is_commit_sha(value) => {
                return Err(format!("commit link is invalid: {value}"));
            }
            _ => {}
        }
        Ok(())
    }

    /// The checkout ids every workspace this issue links derives, for the one
    /// reference kind that names a checkout rather than a record.
    ///
    /// Derived from each directory's PATH, the way `external_worktree_id`
    /// mints them everywhere else, so two directories called `bridge` in two
    /// workspaces are two ids and a reference cannot cross between them.
    fn issue_checkout_ids(&self, issue: &Issue) -> std::collections::BTreeSet<String> {
        issue
            .links
            .workspace_ids
            .iter()
            .filter_map(|workspace_id| self.workspaces.get(workspace_id))
            .flat_map(|workspace| {
                workspace
                    .directories
                    .iter()
                    .map(|directory| crate::worktree::external_worktree_id(&directory.path))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// One issue and the project id it belongs to.
    ///
    /// The record holds a path; the wire holds an id. Resolving here means no
    /// verb above ever sees the path, and an issue whose project has been
    /// removed reads as unknown rather than as an issue nobody can act on.
    pub(in crate::app) fn tracker_issue(
        &mut self,
        issue_id: &str,
    ) -> Result<(String, Issue), String> {
        let issue = self
            .tracker_store()?
            .load_tracker_issue(issue_id)
            .stored()?
            .ok_or_else(|| format!("unknown issue_id: {issue_id}"))?;
        let project_id = self
            .projects
            .find_by_canonical_path(std::path::Path::new(&issue.project_path))
            .map(|project| project.id.clone())
            .ok_or_else(|| format!("unknown issue_id: {issue_id}"))?;
        Ok((project_id, issue))
    }

    /// Where a project's issues are stored, by the canonical path that outlives
    /// its `proj-N` id.
    pub(in crate::app) fn tracker_project_path(&self, project_id: &str) -> Result<String, String> {
        self.projects
            .get(project_id)
            .map(|project| project.repo_path.display().to_string())
            .ok_or_else(|| format!("unknown project_id: {project_id}"))
    }

    /// Tell every `changes` subscriber that this project's issues moved.
    ///
    /// Called after the write lands, never before: a subscriber told to refetch
    /// ahead of the commit would read the state the write is about to replace.
    pub(in crate::app) fn note_issues_changed(&self, project_id: &str, issue_id: &str) {
        self.changes
            .note_issues(project_id, &[issue_id.to_string()]);
    }

    /// The store, or why there is none.
    ///
    /// A bridge running without persistence has no tracker: an issue that
    /// vanishes on restart is worse than a tracker that says it is not
    /// available, because the user would file work into it and lose it.
    pub(in crate::app) fn tracker_store(&self) -> Result<&Store, String> {
        self.store
            .as_ref()
            .ok_or_else(|| "issues need a durable store; this bridge has none".to_string())
    }
}

/// Whether a word is a full commit sha: forty lowercase hex digits, the same
/// shape a thread link's commit is held to.
pub(in crate::app) fn is_commit_sha(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

/// A store call's answer as a verb's refusal.
///
/// Exists so the verbs above can say `.stored()?` rather than repeating the
/// same `map_err` at every store call, without an `impl From<StoreError> for
/// String` that would quietly change how every other module in the crate
/// converts one.
pub(in crate::app) trait StoredAnswer<T> {
    fn stored(self) -> Result<T, String>;
}

impl<T> StoredAnswer<T> for Result<T, crate::store::StoreError> {
    fn stored(self) -> Result<T, String> {
        self.map_err(|error| error.to_string())
    }
}
