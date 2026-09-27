//! What an agent may do to its project's tasks (spec: Tasks → The MCP
//! tools).
//!
//! Twelve tools, on the coding and project surfaces alike, each a thin wrapper
//! over the verb of the same shape: the same code path, the same refusals, the
//! same record afterwards. What the wrapper adds is who is calling.
//!
//! Two things are never arguments. **The project** comes from the calling
//! agent's conversation owner binding, so a tool call cannot reach a project
//! this agent does not work on however it is spelled — a call carrying a
//! project id is parsed as though it had not. **The author** is the calling
//! agent, so a comment is signed by whoever wrote it and an event by whoever
//! caused it, rather than by whoever the call claims.

use super::{edits, AssignTarget, StoredAnswer, TaskWrite};
use crate::app::AppState;
use crate::mcp::BridgeAction;
use crate::tracker::{Actor, MAX_BODY_BYTES};
use serde_json::{json, Value};

impl AppState {
    /// The tracker's tools. `None` is "not one of mine", which is every other
    /// action either working surface answers.
    pub(in crate::app) fn task_surface_action(
        &mut self,
        entity_id: &str,
        agent_id: &str,
        action: &BridgeAction,
    ) -> Option<Result<Value, String>> {
        // Resolved once, before any arm runs: an agent whose owner is bound to
        // no project has no tasks to reach, and saying so once beats each arm
        // discovering it.
        let scope = match self.task_scope(entity_id, agent_id) {
            Ok(scope) => scope,
            Err(refusal) => return is_a_task_tool(action).then_some(Err(refusal)),
        };
        let answered = match action {
            BridgeAction::TrackerListTasks {
                state,
                status,
                label,
            } => self.tasks_list(&scope.params(asked(&[
                ("state", state),
                ("status", status),
                ("label", label),
            ]))),
            BridgeAction::TrackerGetTask { task_id } => {
                self.scoped_task_call(&scope, task_id, json!({}), AppState::tasks_get)
            }
            BridgeAction::TrackerReadComment { comment_id } => {
                self.read_comment_as_agent(&scope, comment_id)
            }
            BridgeAction::TrackerCreateTask {
                title,
                body,
                status,
                labels,
                priority,
                attachments,
                notify_user,
                mention_user,
                ..
            } => self.create_task_as_agent(
                &scope,
                title,
                body,
                status,
                labels,
                priority,
                attachments,
                notify_user,
                mention_user,
            ),
            BridgeAction::TrackerCommentTask {
                task_id,
                body,
                refs,
                attachments,
                mention_user,
                notify_user,
                ..
            } => self.comment_task_as_agent(
                &scope,
                task_id,
                body,
                refs,
                attachments,
                CommentAsks {
                    mentions_user: mention_user.unwrap_or(false),
                    notifies_user: notify_user.unwrap_or(false),
                },
            ),
            BridgeAction::TrackerAssignTask {
                task_id,
                assignee,
                note,
                ..
            } => self.assign_task_as_agent(&scope, task_id, assignee, note.clone()),
            BridgeAction::TrackerMoveTask {
                task_id, status, ..
            } => self.move_task_as_agent(&scope, task_id, status),
            BridgeAction::TrackerLabelTask {
                task_id,
                add,
                remove,
                ..
            } => self.label_task_as_agent(&scope, task_id, add, remove),
            BridgeAction::TrackerCloseTask {
                task_id, reason, ..
            } => self.close_task_as_agent(&scope, task_id, reason.clone()),
            BridgeAction::TrackerTrackTask { task_id } => {
                self.set_tracking_as_agent(&scope, task_id, true)
            }
            BridgeAction::TrackerUntrackTask { task_id } => {
                self.set_tracking_as_agent(&scope, task_id, false)
            }
            BridgeAction::TrackerLinkTask {
                task_id,
                workspace_id,
                branch,
                commit,
                conversation_id,
                parent_task_id,
                ..
            } => self.link_task_as_agent(
                &scope,
                task_id,
                asked(&[
                    ("workspace_id", workspace_id),
                    ("branch", branch),
                    ("commit", commit),
                    ("conversation_id", conversation_id),
                    ("parent_task_id", parent_task_id),
                ]),
            ),
            _ => return None,
        };
        let answered = self.also_track(&scope, action, answered);
        Some(self.also_watch(&scope, action, answered))
    }

    /// Honour `track` on a write that carried it (spec: Tasks → Tracking).
    ///
    /// Once, here, rather than threaded through six handlers: the task a
    /// create tracks is one that did not exist when the call was made, so the
    /// only place every write can name its task is its answer. Each of them
    /// answers `{"task": …}`, and the tracked task replaces it so the caller
    /// reads its own `trackers` back without a second call.
    ///
    /// Quiet about its own failure, which can only be the tracker cap: the
    /// write landed and is durable before this runs, and reporting the call as
    /// failed would invite the agent to make it twice.
    fn also_track(
        &mut self,
        scope: &TaskScope,
        action: &BridgeAction,
        answered: Result<Value, String>,
    ) -> Result<Value, String> {
        let mut answered = answered?;
        if !wants_tracking(action) {
            return Ok(answered);
        }
        let Some(task_id) = answered["task"]["id"].as_str().map(str::to_string) else {
            return Ok(answered);
        };
        match self.set_tracking_as_agent(scope, &task_id, true) {
            Ok(tracked) => answered["task"] = tracked["task"].clone(),
            Err(why) => eprintln!("track {task_id} alongside the write: {why}"),
        }
        Ok(answered)
    }

    /// Honour `notify_user` on a write that carried it (spec: Tasks →
    /// Watching).
    ///
    /// Beside `also_track` and for the same reason: the task a create
    /// concerns does not exist until its answer names it. Where tracking puts
    /// the AGENT on the task, this puts the task in the USER's inbox — the
    /// agent saying "you asked for this, so you should see it".
    ///
    /// Quiet about its own failure. The write landed and is durable; reporting
    /// the call as failed would invite the agent to make it twice.
    fn also_watch(
        &mut self,
        scope: &TaskScope,
        action: &BridgeAction,
        answered: Result<Value, String>,
    ) -> Result<Value, String> {
        let mut answered = answered?;
        if !wants_the_user_told(action) {
            return Ok(answered);
        }
        let Some(task_id) = answered["task"]["id"].as_str().map(str::to_string) else {
            return Ok(answered);
        };
        let Ok((project_id, task)) = self.tracker_task(&task_id) else {
            return Ok(answered);
        };
        match self.set_watching(&project_id, task, true, scope.actor.clone()) {
            Ok(watched) => answered["task"] = watched["task"].clone(),
            Err(why) => eprintln!("show {task_id} to the user alongside the write: {why}"),
        }
        Ok(answered)
    }

    /// Which project's tasks this agent reaches, and whose name goes on what
    /// it does.
    fn task_scope(&self, entity_id: &str, agent_id: &str) -> Result<TaskScope, String> {
        let project_id = self
            .projects
            .project_id_of(entity_id)
            .map(str::to_string)
            .ok_or_else(|| format!("{entity_id} belongs to no project"))?;
        Ok(TaskScope {
            project_id,
            entity_id: entity_id.to_string(),
            actor: Actor::Agent {
                agent_id: agent_id.to_string(),
            },
        })
    }

    /// A read or write about one task, refused unless that task is this
    /// agent's project's.
    ///
    /// A task of another project reads as unknown rather than as forbidden:
    /// the agent cannot list it, cannot have been handed it, and telling it
    /// that an id it guessed exists somewhere else says more than it asked.
    fn scoped_task_call(
        &mut self,
        scope: &TaskScope,
        task_id: &str,
        mut params: Value,
        call: fn(&mut AppState, &Value) -> Result<Value, String>,
    ) -> Result<Value, String> {
        self.task_of_this_agents_project(scope, task_id)?;
        params["task_id"] = json!(task_id);
        call(self, &params)
    }

    /// The task, when it is one this agent may act on.
    fn task_of_this_agents_project(
        &mut self,
        scope: &TaskScope,
        task_id: &str,
    ) -> Result<crate::tracker::Task, String> {
        let (project_id, task) = self.tracker_task(task_id)?;
        if project_id != scope.project_id {
            return Err(format!("unknown task_id: {task_id}"));
        }
        Ok(task)
    }

    /// One comment, with enough of its task to know what it is about.
    ///
    /// Scoped like every other read here: a comment on another project's task
    /// is refused by the same words an unknown id gets, because to this agent
    /// those are the same thing.
    fn read_comment_as_agent(
        &mut self,
        scope: &TaskScope,
        comment_id: &str,
    ) -> Result<Value, String> {
        let unknown = || format!("There is no comment {comment_id} on this project's tasks.");
        let comment = self
            .tracker_store()?
            .load_tracker_comment(comment_id)
            .stored()?
            .ok_or_else(unknown)?;
        let (project_id, task) = self.tracker_task(&comment.task_id).map_err(|_| unknown())?;
        if project_id != scope.project_id {
            return Err(unknown());
        }
        let mut read = json!({
            "comment_id": comment.id,
            "task_id": task.id,
            // The number and title, because the notice gave a number and the
            // reader is about to answer about it.
            "number": task.number,
            "title": task.title,
            "author": comment.author,
            "created_at": comment.created_at,
            "body": comment.body,
            "refs": comment.refs,
        });
        if comment.mentions_user {
            read["mentions_user"] = json!(true);
        }
        if comment.notifies_user {
            read["notifies_user"] = json!(true);
        }
        if let Some(reading) = &comment.author_context {
            read["author_context"] = json!(reading);
        }
        if let Some(line) = self.author_context_line(scope, &comment) {
            read["author_context_line"] = json!(line);
        }
        Ok(read)
    }

    /// The sentence the PROJECT agent reads about a comment's author: how full
    /// its context was as it wrote (#68). A comment reaches the project agent
    /// as a one-line notice naming only its id, so this is where the line
    /// lands. `None` for any other reader, and for a comment with no reading.
    fn author_context_line(
        &self,
        scope: &TaskScope,
        comment: &crate::tracker::TaskComment,
    ) -> Option<String> {
        let reader = scope.actor.agent_id()?;
        if !crate::agent::is_project_agent(reader) {
            return None;
        }
        let reading = comment.author_context.as_ref()?;
        let author = self
            .agent_display_name(&comment.author)
            .unwrap_or_else(|| format!("Agent {}", comment.author.agent_id().unwrap_or_default()));
        Some(reading.sentence(&author))
    }

    #[allow(clippy::too_many_arguments)]
    fn create_task_as_agent(
        &mut self,
        scope: &TaskScope,
        title: &str,
        body: &Option<String>,
        status: &Option<String>,
        labels: &[String],
        priority: &Option<String>,
        attachments: &[Value],
        notify_user: &Option<bool>,
        mention_user: &Option<bool>,
    ) -> Result<Value, String> {
        let project_path = self.tracker_project_path(&scope.project_id)?;
        let now = crate::store::now_rfc3339();
        let mut params = asked(&[("body", body), ("status", status), ("priority", priority)]);
        params["title"] = json!(title);
        params["labels"] = json!(labels);
        if let Some(asked) = notify_user {
            params["notify_user"] = json!(asked);
        }
        let mut draft = edits::drafted_task(&params, &project_path, scope.actor.clone(), &now)?;
        // Taken in only once the rest of the filing has been read: a refused
        // title should not leave a copied recording behind it.
        params["attachments"] = json!(self.take_in_agent_files(attachments)?);
        draft.attachments = self.parse_task_attachments(&params)?;
        // Whether the user hears about a task an AGENT filed. Two ways to
        // say yes: the agent asked for it with `notify_user`, because the user
        // asked for the task; or the device says every agent-filed task is
        // worth seeing, which is the default. An agent filing for another
        // agent, on a device that has turned that off, is not the user's
        // business until somebody says it is.
        draft.watched = mention_user.unwrap_or(false)
            || notify_user_asked(&params)
            || self.watch_agent_filed_tasks;
        let mut created = crate::tracker::TaskEvent::new(
            &draft.id,
            scope.actor.clone(),
            crate::tracker::TaskEventKind::Created,
            json!({ "title": draft.title }),
            &now,
        );
        created.mentions_user = mention_user.unwrap_or(false);
        self.capture_task_identities(
            &mut draft,
            &[crate::tracker::TimelineEntry::Event(created.clone())],
        );
        let task = self
            .tracker_store()?
            .create_tracker_task(draft, std::slice::from_ref(&created))
            .stored()?;
        self.note_tasks_changed(&scope.project_id, &task.id);
        // A create does not pass through `commit_task_write` — the number is
        // minted inside the insert's own transaction — so what that funnel does
        // for an agent is done here by hand: the push when the filing asks the
        // user to read it (#189, #191), and the agent's own account of it.
        let mut write = TaskWrite::by(scope.actor.clone(), task.clone());
        write.events.push(created);
        self.push_task_news(&write);
        self.say_what_the_agent_did(&write);
        Ok(json!({
            "task": super::task_json(&scope.project_id, &task),
        }))
    }

    fn comment_task_as_agent(
        &mut self,
        scope: &TaskScope,
        task_id: &str,
        body: &str,
        refs: &[crate::thread::ThreadLink],
        attachments: &[Value],
        asks: CommentAsks,
    ) -> Result<Value, String> {
        let task = self.task_of_this_agents_project(scope, task_id)?;
        let params = json!({ "body": body, "refs": refs });
        let body = edits::required_text(&params, "body", MAX_BODY_BYTES)?;
        let refs = super::refs::fenced_refs(&params, &task, &self.task_checkout_ids(&task))?;
        let attachments = self.parse_task_attachments(
            &json!({ "attachments": self.take_in_agent_files(attachments)? }),
        )?;
        let now = crate::store::now_rfc3339();
        let comment = crate::tracker::TaskComment {
            id: crate::tracker::new_comment_id(),
            task_id: task.id.clone(),
            author: scope.actor.clone(),
            body,
            mentions_user: asks.mentions_user,
            notifies_user: asks.notifies_user,
            refs,
            attachments,
            created_at: now.clone(),
            author_context: scope
                .actor
                .agent_id()
                .and_then(|agent_id| self.agent_context_reading(&scope.entity_id, agent_id)),
        };
        let mut write = TaskWrite::by(scope.actor.clone(), task);
        write.comments.push(comment.clone());
        let answered = self.commit_task_write(&scope.project_id, write, &now)?;
        Ok(json!({
            "task": answered["task"],
            "comment": serde_json::to_value(&comment).map_err(|error| error.to_string())?,
        }))
    }

    fn assign_task_as_agent(
        &mut self,
        scope: &TaskScope,
        task_id: &str,
        assignee: &Value,
        note: Option<String>,
    ) -> Result<Value, String> {
        let task = self.task_of_this_agents_project(scope, task_id)?;
        // A TOOL spells the agent's choice `harness`; the daemon's parse reads
        // `provider`, which is what the wire and `agent.add` call it. Mapped
        // here, at the one boundary where the two words meet.
        let target = AssignTarget::parse(Some(&provider_for_harness(assignee)))?;
        let sender = crate::app::AgentSender {
            entity_id: &scope.entity_id,
            agent_id: scope.actor.agent_id().unwrap_or_default(),
        };
        self.assign_task_to(
            &scope.project_id,
            task,
            target,
            note,
            scope.actor.clone(),
            Some(sender),
        )
    }

    fn move_task_as_agent(
        &mut self,
        scope: &TaskScope,
        task_id: &str,
        status: &str,
    ) -> Result<Value, String> {
        let task = self.task_of_this_agents_project(scope, task_id)?;
        let params = json!({ "status": status });
        let status = edits::optional_status(&params, "status")?
            .ok_or_else(|| "status is required".to_string())?;
        let now = crate::store::now_rfc3339();
        let mut write = TaskWrite::by(scope.actor.clone(), task);
        edits::move_to(&mut write, &status, &scope.actor, json!({}), &now);
        self.commit_task_write(&scope.project_id, write, &now)
    }

    fn label_task_as_agent(
        &mut self,
        scope: &TaskScope,
        task_id: &str,
        add: &[String],
        remove: &[String],
    ) -> Result<Value, String> {
        use crate::tracker::normalize_labels;

        let task = self.task_of_this_agents_project(scope, task_id)?;
        // Validate both requested lists even when the requested change would
        // leave the stored set alone. This is the create_task validator.
        let add = normalize_labels(add)?;
        let remove = normalize_labels(remove)?;
        let mut labels: Vec<String> = task
            .labels
            .iter()
            .filter(|label| !remove.iter().any(|word| word.eq_ignore_ascii_case(label)))
            .cloned()
            .collect();
        for label in add {
            if !labels.iter().any(|word| word.eq_ignore_ascii_case(&label)) {
                labels.push(label);
            }
        }
        let labels = normalize_labels(&labels)?;
        if labels == task.labels {
            return Ok(json!({
                "labels": task.labels,
                "task": super::task_json(&scope.project_id, &task),
            }));
        }
        let now = crate::store::now_rfc3339();
        let mut write = TaskWrite::by(scope.actor.clone(), task);
        edits::apply_update(&mut write, &json!({ "labels": labels }), &scope.actor, &now)?;
        let mut answer = self.commit_task_write(&scope.project_id, write, &now)?;
        answer["labels"] = answer["task"]["labels"].clone();
        Ok(answer)
    }

    fn close_task_as_agent(
        &mut self,
        scope: &TaskScope,
        task_id: &str,
        reason: Option<String>,
    ) -> Result<Value, String> {
        let task = self.task_of_this_agents_project(scope, task_id)?;
        if !task.is_open() {
            return Err(format!("task #{} is already closed", task.number));
        }
        let now = crate::store::now_rfc3339();
        let mut write = TaskWrite::by(scope.actor.clone(), task);
        edits::close(&mut write, &scope.actor, reason, &now);
        self.commit_task_write(&scope.project_id, write, &now)
    }

    /// `track_task` / `untrack_task` — the CALLER starts or stops watching.
    ///
    /// The agent is the caller and never an argument: a tool that could
    /// subscribe somebody else would be one agent deciding what another is
    /// woken for, which is not its to decide.
    fn set_tracking_as_agent(
        &mut self,
        scope: &TaskScope,
        task_id: &str,
        tracking: bool,
    ) -> Result<Value, String> {
        let task = self.task_of_this_agents_project(scope, task_id)?;
        let Some(agent_id) = scope.actor.agent_id().map(str::to_string) else {
            return Err("only an agent can track a task".to_string());
        };
        self.set_tracking(
            &scope.project_id,
            task,
            &agent_id,
            tracking,
            scope.actor.clone(),
            None,
        )
    }

    fn link_task_as_agent(
        &mut self,
        scope: &TaskScope,
        task_id: &str,
        params: Value,
    ) -> Result<Value, String> {
        let task = self.task_of_this_agents_project(scope, task_id)?;
        let asked = edits::asked_links(&params)?;
        let now = crate::store::now_rfc3339();
        let mut write = TaskWrite::by(scope.actor.clone(), task);
        self.apply_links(&scope.project_id, &mut write, &asked, &scope.actor, &now)?;
        self.commit_task_write(&scope.project_id, write, &now)
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
/// What an agent's comment asked of the user, kept on the comment: a mention
/// (`mention_user`) and a notice (`notify_user`). Either one also watches the
/// task ([`wants_the_user_told`]).
#[derive(Clone, Copy)]
struct CommentAsks {
    mentions_user: bool,
    notifies_user: bool,
}

/// Whether a tool call asked for the user to be told. Absent is no.
pub(in crate::app) fn notify_user_asked(params: &Value) -> bool {
    params
        .get("notify_user")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn asked(fields: &[(&str, &Option<String>)]) -> Value {
    let mut params = json!({});
    for (key, value) in fields {
        if let Some(value) = value {
            params[*key] = json!(value);
        }
    }
    params
}

/// Whether this action is one of the tracker's twelve.
///
/// Asked only where the scope could not be resolved at all: an agent whose
/// owner is bound to no project has no tasks to reach, and must still be able
/// to call every tool that is not about a project.
/// Whether this write asked for the user to be told about the task.
///
/// A create answers for itself — the draft is already marked watched or not,
/// by the flag or the device setting — so only the writes that touch a task
/// that already exists are here.
fn wants_the_user_told(action: &BridgeAction) -> bool {
    match action {
        BridgeAction::TrackerCommentTask {
            notify_user,
            mention_user,
            ..
        } => notify_user.unwrap_or(false) || mention_user.unwrap_or(false),
        BridgeAction::TrackerAssignTask { notify_user, .. } => notify_user.unwrap_or(false),
        _ => false,
    }
}

/// Whether this write was asked to follow the task it touched.
///
/// Absent means NO everywhere but `create_task`, where it means yes. An agent
/// that moves somebody else's card in passing has not asked to hear about it
/// ever again; an agent that files a task almost always wants to know how it
/// goes, and the one that filed and assigned twelve in an afternoon heard
/// nothing about any of them.
fn wants_tracking(action: &BridgeAction) -> bool {
    match action {
        BridgeAction::TrackerCreateTask { track, .. } => track.unwrap_or(true),
        BridgeAction::TrackerCommentTask { track, .. }
        | BridgeAction::TrackerAssignTask { track, .. }
        | BridgeAction::TrackerMoveTask { track, .. }
        | BridgeAction::TrackerLabelTask { track, .. }
        | BridgeAction::TrackerCloseTask { track, .. }
        | BridgeAction::TrackerLinkTask { track, .. } => track.unwrap_or(false),
        _ => false,
    }
}

fn is_a_task_tool(action: &BridgeAction) -> bool {
    matches!(
        action,
        BridgeAction::TrackerListTasks { .. }
            | BridgeAction::TrackerGetTask { .. }
            | BridgeAction::TrackerReadComment { .. }
            | BridgeAction::TrackerCreateTask { .. }
            | BridgeAction::TrackerCommentTask { .. }
            | BridgeAction::TrackerAssignTask { .. }
            | BridgeAction::TrackerMoveTask { .. }
            | BridgeAction::TrackerLabelTask { .. }
            | BridgeAction::TrackerCloseTask { .. }
            | BridgeAction::TrackerLinkTask { .. }
            | BridgeAction::TrackerTrackTask { .. }
            | BridgeAction::TrackerUntrackTask { .. }
    )
}

/// Who is calling, and what that lets them reach.
struct TaskScope {
    project_id: String,
    /// The calling agent's conversation owner, for a delivery it asks for.
    entity_id: String,
    actor: Actor,
}

impl TaskScope {
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
