//! The per-project task tracker's verbs (spec: Tasks).
//!
//! NOT `app/tasks/`, which is the retired plan-and-stages flow. The two share
//! the English word and nothing else: different tables, different verbs, and
//! neither reads the other.
//!
//! Every verb here is the same three steps — resolve the task (or the
//! project), decide what changed and what the timeline should say about it,
//! then write the record and its events in one store call and tell the project
//! something moved. The deciding is [`edits`]; the shape a client reads is
//! [`views`]; the fencing an agent-supplied reference goes through is
//! [`refs`].

mod activity;
mod agent_files;
mod attachments;
mod dispatch;
mod edits;
mod identities;
mod inbox;
mod notices;
mod pages;
mod push;
mod refs;
mod reminder;
mod said;
mod tools;
mod tracking;
mod views;

#[cfg(test)]
pub(in crate::app) use agent_files::between_check_and_copy;
pub use agent_files::AGENT_ATTACHMENT_MAX_BYTES;
pub use attachments::ATTACHMENT_READ_CHUNK_BYTES;

pub(in crate::app) use dispatch::AssignTarget;
use identities::StoredRosters;
#[cfg(test)]
pub(in crate::app) use inbox::unread_since_mark;
pub(in crate::app) use views::{columns_json, task_json, task_with_timeline_json};

use crate::app::{require_str, AppState};
use crate::store::{Store, TaskFilter, TaskSeek};
use crate::tracker::{
    Actor, Task, TaskComment, TaskEvent, TaskEventKind, TaskState, MAX_BODY_BYTES,
};
use serde_json::{json, Value};

/// What a verb is about to write: the record as it now stands, and everything
/// the timeline should say about how it got there.
///
/// Carried as one value because the two are one write — a refusal must not
/// leave a timeline claiming a move the record does not show — and because
/// every verb builds the same shape.
pub(in crate::app) struct TaskWrite {
    pub(in crate::app) task: Task,
    pub(in crate::app) comments: Vec<TaskComment>,
    pub(in crate::app) events: Vec<TaskEvent>,
    /// Who is making this change.
    ///
    /// Carried on the write rather than passed beside it: a tracking notice
    /// must not go back to whoever caused it, and that exclusion is the rule
    /// the whole feature rests on. An argument every caller had to remember to
    /// keep in step would be one a caller could get wrong.
    pub(in crate::app) actor: Actor,
}

impl TaskWrite {
    /// A write, and who is making it.
    fn by(actor: Actor, task: Task) -> TaskWrite {
        TaskWrite {
            task,
            comments: Vec::new(),
            events: Vec::new(),
            actor,
        }
    }

    /// Whether this write moves a task that links a workspace to Done or
    /// closes it: that workspace may have just become reclaimable.
    fn finishes_a_workspace_task(&self) -> bool {
        !self.task.links.workspace_ids.is_empty()
            && self.events.iter().any(|event| match event.kind {
                TaskEventKind::Closed => true,
                TaskEventKind::Moved => {
                    event.payload.get("to").and_then(Value::as_str)
                        == Some(crate::tracker::DONE_STATUS)
                }
                _ => false,
            })
    }

    pub(in crate::app) fn event(
        &mut self,
        actor: &Actor,
        kind: TaskEventKind,
        payload: Value,
        now: &str,
    ) {
        self.events.push(TaskEvent::new(
            &self.task.id,
            actor.clone(),
            kind,
            payload,
            now,
        ));
    }
}

impl AppState {
    // ------------------------------------------------------------- reads ---

    /// `tasks.list` — one project's tasks, newest first.
    ///
    /// `state` and `status` narrow the store read; `assignee` and `label` are
    /// applied to each row as it is read, because both live inside the record
    /// and hoisting a label list would mean a join table phase 1 does not need.
    ///
    /// Every row is what `tasks.get` says of that task, gathered in bulk:
    /// the timelines in one read, and the store's agent records at most once.
    /// Read per task, a long project's list held the app lock — and every
    /// other call behind it — for seconds (#128).
    ///
    /// With a `limit` it answers one page, and `next_cursor` while there is
    /// another (#85, [`pages`]). Only the page's own timelines are read.
    pub(crate) fn tasks_list(&mut self, params: &Value) -> Result<Value, String> {
        let project_id = require_str(params, "project_id")?;
        let project_path = self.tracker_project_path(&project_id)?;
        let state = edits::optional_state(params)?;
        let status = edits::optional_status(params, "status")?;
        let assignee = edits::optional_assignee_filter(params)?;
        let label = crate::app::optional_nonempty_string(params, "label")?;
        let store_key = self.tracker_store()?.tracker_list_key().stored()?;
        let page = pages::PageAsk::parse(
            params,
            &pages::ListOf {
                store_key: &store_key,
                project_path: &project_path,
            },
            &pages::ListFilter {
                state,
                status: status.as_deref(),
                assignee: &assignee,
                label,
            },
        )?;
        let stretch = self
            .tracker_store()?
            .list_tracker_tasks_below(
                &project_path,
                TaskFilter {
                    state,
                    status: status.as_deref(),
                },
                TaskSeek {
                    below: page.below,
                    take: page.rows_to_keep(),
                    scan: page.rows_to_scan(),
                },
                |task| assignee.matches(task) && edits::carries_label(task, label),
            )
            .stored()?;
        let mut tasks = stretch.tasks;
        let next_cursor = page.cut(&mut tasks, stretch.scanned_to, |task| task.number);
        let rows = self.listed_rows(&project_id, tasks)?;
        let mut answer = json!({
            "project_id": project_id,
            "tasks": rows,
            // Device-wide, and here because the Done section reads it beside
            // the list: every push that moves a task re-reads this answer,
            // and a new session is pushed as one (`note_user_activity`).
            "user_session": self.user_session_json(),
        });
        if let Some(next_cursor) = next_cursor {
            answer["next_cursor"] = Value::from(next_cursor);
        }
        Ok(answer)
    }

    /// Each listed task as `tasks.get` says it, the timelines read in one go.
    fn listed_rows(&mut self, project_id: &str, tasks: Vec<Task>) -> Result<Vec<Value>, String> {
        let ids: Vec<String> = tasks.iter().map(|task| task.id.clone()).collect();
        let mut timelines = self
            .tracker_store()?
            .load_tracker_timelines(&ids)
            .stored()?;
        let rosters = StoredRosters::default();
        let mut rows = Vec::with_capacity(tasks.len());
        for task in tasks {
            let timeline = timelines.remove(&task.id).unwrap_or_default();
            let task = self.backfill_task_identities(task, &timeline, &rosters)?;
            let task = self.backfill_done_at(task, &timeline)?;
            let task = self.task_with_read_identities(task, &timeline, &rosters);
            rows.push(views::read_task_json(project_id, &task, &timeline));
        }
        Ok(rows)
    }

    /// `tasks.get` — one task and its whole timeline.
    pub(crate) fn tasks_get(&mut self, params: &Value) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        let (project_id, task) = self.tracker_task(&task_id)?;
        let timeline = self
            .tracker_store()?
            .load_tracker_timeline(&task.id)
            .stored()?;
        let rosters = StoredRosters::default();
        let task = self.backfill_task_identities(task, &timeline, &rosters)?;
        let task = self.backfill_done_at(task, &timeline)?;
        let task = self.task_with_read_identities(task, &timeline, &rosters);
        Ok(task_with_timeline_json(&project_id, &task, &timeline))
    }

    /// Give a task filed before `done_at` existed the one its timeline
    /// says, and keep it, so the next write carries it too.
    fn backfill_done_at(
        &self,
        mut task: Task,
        timeline: &[crate::tracker::TimelineEntry],
    ) -> Result<Task, String> {
        let done_at = crate::tracker::done_at_from_timeline(&task, timeline);
        if task.done_at.is_none() && done_at.is_some() {
            task.done_at = done_at;
            self.tracker_store()?
                .save_tracker_task_activity(&task, &[], &[])
                .stored()?;
        }
        Ok(task)
    }

    /// `tasks.columns` — the board's columns, in board order.
    ///
    /// Takes a project it does not read, so the verb does not have to change
    /// when columns become per-project. A project that is not registered is
    /// still refused: answering for one is saying it exists.
    pub(crate) fn tasks_columns(&mut self, params: &Value) -> Result<Value, String> {
        let project_id = require_str(params, "project_id")?;
        self.tracker_project_path(&project_id)?;
        Ok(columns_json(&project_id))
    }

    // ------------------------------------------------------------ writes ---

    /// `tasks.create` — file one, mint its number, say it was created, and
    /// hand it over when it was filed with an assignee.
    ///
    /// Filing and assigning are one call because they are one thought: most
    /// tasks an agent files are for somebody, and making the client do two
    /// round trips would leave a task assigned to nobody in between for every
    /// failure of the second. The assignment is the WHOLE of `tasks.assign` —
    /// the same delivery, the same events, the same deferral when it cuts a
    /// checkout — so there is one answer to what assigning means.
    pub(crate) fn tasks_create(&mut self, params: &Value) -> Result<Value, String> {
        let project_id = require_str(params, "project_id")?;
        let project_path = self.tracker_project_path(&project_id)?;
        let actor = Actor::User;
        let now = crate::store::now_rfc3339();
        let mut draft = edits::drafted_task(params, &project_path, actor.clone(), &now)?;
        // Before the write, like the assignee below it: a file this bridge
        // cannot resolve must refuse the whole call rather than leave a filed
        // task whose reason for being filed is missing from it.
        draft.attachments = self.parse_task_attachments(params)?;
        // The user filed it, so the user watches it. Nobody has to ask.
        draft.watched = true;
        // Read BEFORE the task is written: an assignee this bridge cannot make
        // sense of must refuse the whole call rather than leave a filed task
        // nobody asked for.
        let target = match params.get("assignee") {
            None | Some(Value::Null) => None,
            Some(assignee) => Some(AssignTarget::parse(Some(assignee))?),
        };
        let created = TaskEvent::new(
            &draft.id,
            actor.clone(),
            TaskEventKind::Created,
            json!({ "title": draft.title }),
            &now,
        );
        self.capture_task_identities(
            &mut draft,
            &[crate::tracker::TimelineEntry::Event(created.clone())],
        );
        let task = self
            .tracker_store()?
            .create_tracker_task(draft, &[created])
            .stored()?;
        self.note_tasks_changed(&project_id, &task.id);
        let Some(target) = target else {
            return Ok(json!({
                "task": task_json(&project_id, &task),
                "dispatch": Value::Null,
            }));
        };
        let note = crate::app::optional_nonempty_string(params, "note")?.map(str::to_string);
        self.assign_task_to(&project_id, task, target, note, actor, None)
    }

    /// `tasks.update` — title, body, labels, priority, status, state.
    ///
    /// Only the fields present are applied, and each one that actually changes
    /// something writes its own event. A title, a body or a priority writes
    /// none: `updated_at` is the whole history those need.
    pub(crate) fn tasks_update(&mut self, params: &Value) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        let (project_id, task) = self.tracker_task(&task_id)?;
        let now = crate::store::now_rfc3339();
        let mut write = TaskWrite::by(Actor::User, task);
        edits::apply_update(&mut write, params, &Actor::User, &now)?;
        self.commit_task_write(&project_id, write, &now)
    }

    /// `tasks.comment` — say something, with typed references fenced twice.
    pub(crate) fn tasks_comment(&mut self, params: &Value) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        let (project_id, task) = self.tracker_task(&task_id)?;
        let body = edits::required_text(params, "body", MAX_BODY_BYTES)?;
        let refs = refs::fenced_refs(params, &task, &self.task_checkout_ids(&task))?;
        let attachments = self.parse_task_attachments(params)?;
        let now = crate::store::now_rfc3339();
        let comment = TaskComment {
            id: crate::tracker::new_comment_id(),
            task_id: task.id.clone(),
            author: Actor::User,
            body,
            mentions_user: false,
            notifies_user: false,
            refs,
            attachments,
            created_at: now.clone(),
            author_context: None,
        };
        let mut write = TaskWrite::by(Actor::User, task);
        write.comments.push(comment.clone());
        // Saying something on a task is caring about it, so the user watches
        // it from here on. Folded into this write rather than done after it:
        // one change to the record, one push, one notice.
        if write.task.set_watched(true) {
            write.event(&Actor::User, TaskEventKind::Watched, json!({}), &now);
        }
        let answered = self.commit_task_write(&project_id, write, &now)?;
        Ok(json!({
            "task": answered["task"],
            "comment": serde_json::to_value(&comment).map_err(|error| error.to_string())?,
        }))
    }

    /// `tasks.link` — one or more of the five link keys, each writing a
    /// `linked` event for the link that was not already there.
    pub(crate) fn tasks_link(&mut self, params: &Value) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        let (project_id, task) = self.tracker_task(&task_id)?;
        let asked = edits::asked_links(params)?;
        let mut write = TaskWrite::by(Actor::User, task);
        let now = crate::store::now_rfc3339();
        self.apply_links(&project_id, &mut write, &asked, &Actor::User, &now)?;
        self.commit_task_write(&project_id, write, &now)
    }

    /// `tasks.close` — closing an already closed task is a conflict, not a
    /// silent no-op: the caller believed something that was not true.
    pub(crate) fn tasks_close(&mut self, params: &Value) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        let (project_id, task) = self.tracker_task(&task_id)?;
        if !task.is_open() {
            return Err(format!("task #{} is already closed", task.number));
        }
        let reason = crate::app::optional_nonempty_string(params, "reason")?.map(str::to_string);
        let now = crate::store::now_rfc3339();
        let mut write = TaskWrite::by(Actor::User, task);
        edits::close(&mut write, &Actor::User, reason, &now);
        self.commit_task_write(&project_id, write, &now)
    }

    /// `tasks.reopen` — the same rule the other way.
    pub(crate) fn tasks_reopen(&mut self, params: &Value) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        let (project_id, task) = self.tracker_task(&task_id)?;
        if task.is_open() {
            return Err(format!("task #{} is already open", task.number));
        }
        let now = crate::store::now_rfc3339();
        let mut write = TaskWrite::by(Actor::User, task);
        write.task.state = TaskState::Open;
        write.task.closed_at = None;
        write.event(&Actor::User, TaskEventKind::Reopened, json!({}), &now);
        self.commit_task_write(&project_id, write, &now)
    }

    // ------------------------------------------------------------ shared ---

    /// Write a record and everything its timeline says about the change, then
    /// tell the project it moved.
    ///
    /// The one place a tracker write lands, so `updated_at`, the store call and
    /// the push note cannot be done three different ways by ten verbs.
    pub(in crate::app) fn commit_task_write(
        &mut self,
        project_id: &str,
        mut write: TaskWrite,
        now: &str,
    ) -> Result<Value, String> {
        write.task.updated_at = now.to_string();
        let mut timeline = self
            .tracker_store()?
            .load_tracker_timeline(&write.task.id)
            .stored()?;
        timeline.extend(
            write
                .comments
                .iter()
                .cloned()
                .map(crate::tracker::TimelineEntry::Comment)
                .chain(
                    write
                        .events
                        .iter()
                        .cloned()
                        .map(crate::tracker::TimelineEntry::Event),
                )
                .collect::<Vec<_>>(),
        );
        self.capture_task_identities(&mut write.task, &timeline);
        self.tracker_store()?
            .save_tracker_task_activity(&write.task, &write.comments, &write.events)
            .stored()?;
        self.note_tasks_changed(project_id, &write.task.id);
        if write.finishes_a_workspace_task() {
            self.nudge_workspace_reclaim();
        }
        // AFTER the write is durable, and quiet about its own failure: the
        // change landed, and a conversation that could not be written must not
        // turn it back into a refusal.
        self.notify_trackers(&write);
        // And the user's browsers, when the write adds to their badge (#191).
        self.push_task_news(&write);
        // And the agent says, in its own conversation, what it just did.
        self.say_what_the_agent_did(&write);
        let task = self.task_with_read_identities(write.task, &timeline, &StoredRosters::default());
        Ok(json!({ "task": task_json(project_id, &task) }))
    }

    /// One timeline entry about a linked workspace, without waking anybody.
    ///
    /// What happens to a task's workspace (#135) — Build noticing it went
    /// quiet, Build dropping its build output, somebody reclaiming it — is
    /// bookkeeping about the workspace, not a change to the task. The record
    /// is saved as it stands, so the task keeps its place in every list, and
    /// its watchers are not told: the project agent hears about quiet
    /// workspaces in one notice for the whole sweep, and whoever reclaimed one
    /// already knows.
    pub(in crate::app) fn record_quiet_event(
        &mut self,
        task_id: &str,
        actor: &Actor,
        kind: TaskEventKind,
        payload: Value,
    ) -> Result<(), String> {
        let (project_id, task) = self.tracker_task(task_id)?;
        let now = crate::store::now_rfc3339();
        let event = TaskEvent::new(&task.id, actor.clone(), kind, payload, &now);
        self.tracker_store()?
            .save_tracker_task_activity(&task, &[], &[event])
            .stored()?;
        self.note_tasks_changed(&project_id, &task.id);
        Ok(())
    }

    /// Add the links asked for, each checked against the task's own project
    /// and each writing one `linked` event.
    pub(in crate::app) fn apply_links(
        &mut self,
        project_id: &str,
        write: &mut TaskWrite,
        asked: &edits::AskedLinks,
        actor: &Actor,
        now: &str,
    ) -> Result<(), String> {
        for (kind, value) in asked.entries() {
            self.refuse_foreign_link(project_id, kind, value)?;
            let added = match kind {
                "workspace_id" => {
                    crate::tracker::TaskLinks::add(&mut write.task.links.workspace_ids, value)
                }
                "branch" => crate::tracker::TaskLinks::add(&mut write.task.links.branches, value),
                "commit" => crate::tracker::TaskLinks::add(&mut write.task.links.commits, value),
                "conversation_id" => {
                    crate::tracker::TaskLinks::add(&mut write.task.links.conversation_ids, value)
                }
                _ => self.link_parent(write, value)?,
            };
            if added {
                write.event(actor, TaskEventKind::Linked, json!({ kind: value }), now);
            }
        }
        Ok(())
    }

    /// A parent is one task of the same project, never the task itself and
    /// never a link that closes a loop: a cycle makes "the tasks under this
    /// one" a question with no answer.
    fn link_parent(&mut self, write: &mut TaskWrite, parent_id: &str) -> Result<bool, String> {
        if parent_id == write.task.id {
            return Err("a task cannot be its own parent".to_string());
        }
        if write.task.links.parent_task_id.as_deref() == Some(parent_id) {
            return Ok(false);
        }
        self.refuse_parent_cycle(&write.task, parent_id)?;
        write.task.links.parent_task_id = Some(parent_id.to_string());
        Ok(true)
    }

    /// Walk up from the proposed parent: reaching this task would close a
    /// loop. Bounded by the chain it walks, which a refusal keeps acyclic.
    fn refuse_parent_cycle(&mut self, task: &Task, parent_id: &str) -> Result<(), String> {
        let mut at = Some(parent_id.to_string());
        let mut seen = 0usize;
        while let Some(id) = at {
            if id == task.id {
                return Err(format!(
                    "task {parent_id} is already below this one — a parent link cannot close a loop"
                ));
            }
            let Some(next) = self.tracker_store()?.load_tracker_task(&id).stored()? else {
                return Err(format!("unknown parent_task_id: {parent_id}"));
            };
            if next.project_path != task.project_path {
                return Err(format!("task {parent_id} is not in this task's project"));
            }
            seen += 1;
            if seen > crate::tracker::MAX_LINKS_PER_KIND {
                return Err("parent chain is too deep".to_string());
            }
            at = next.links.parent_task_id;
        }
        Ok(())
    }

    /// Refuse a link that names something of another project. A link is what
    /// a task is about, and a task is about its own project.
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

    /// The checkout ids every workspace this task links derives, for the one
    /// reference kind that names a checkout rather than a record.
    ///
    /// Derived from each directory's PATH, the way `external_worktree_id`
    /// mints them everywhere else, so two directories called `bridge` in two
    /// workspaces are two ids and a reference cannot cross between them.
    fn task_checkout_ids(&self, task: &Task) -> std::collections::BTreeSet<String> {
        task.links
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

    /// One task and the project id it belongs to.
    ///
    /// The record holds a path; the wire holds an id. Resolving here means no
    /// verb above ever sees the path, and a task whose project has been
    /// removed reads as unknown rather than as a task nobody can act on.
    pub(in crate::app) fn tracker_task(&mut self, task_id: &str) -> Result<(String, Task), String> {
        let task = self
            .tracker_store()?
            .load_tracker_task(task_id)
            .stored()?
            .ok_or_else(|| format!("unknown task_id: {task_id}"))?;
        let project_id = self
            .projects
            .find_by_canonical_path(std::path::Path::new(&task.project_path))
            .map(|project| project.id.clone())
            .ok_or_else(|| format!("unknown task_id: {task_id}"))?;
        Ok((project_id, task))
    }

    /// Where a project's tasks are stored, by the canonical path that outlives
    /// its `proj-N` id.
    pub(in crate::app) fn tracker_project_path(&self, project_id: &str) -> Result<String, String> {
        self.projects
            .get(project_id)
            .map(|project| project.repo_path.display().to_string())
            .ok_or_else(|| format!("unknown project_id: {project_id}"))
    }

    /// Tell every `changes` subscriber that this project's tasks moved.
    ///
    /// Called after the write lands, never before: a subscriber told to refetch
    /// ahead of the commit would read the state the write is about to replace.
    pub(in crate::app) fn note_tasks_changed(&self, project_id: &str, task_id: &str) {
        self.changes.note_tasks(project_id, &[task_id.to_string()]);
    }

    /// The store, or why there is none.
    ///
    /// A bridge running without persistence has no tracker: a task that
    /// vanishes on restart is worse than a tracker that says it is not
    /// available, because the user would file work into it and lose it.
    pub(in crate::app) fn tracker_store(&self) -> Result<&Store, String> {
        self.store
            .as_ref()
            .ok_or_else(|| "tasks need a durable store; this bridge has none".to_string())
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
