//! Assignment is dispatch (spec: Tasks → Assignment is dispatch).
//!
//! Handing a task to an agent delivers it into that agent's conversation and
//! starts the agent. There is no second step where somebody turns a task into
//! work, which is the whole reason this file exists rather than
//! `tasks.assign` just writing a name onto a record.
//!
//! Nothing here forks a code path. The workspace is cut by `workspace.create`,
//! the agent is added by the same `agent.add` the project surface's
//! `add_workspace_agent` calls, and the delivery is the post every other
//! message goes through — so a dispatched task is durable, gets an operation
//! receipt, and starts its agent exactly the way a reviewer message does.
//!
//! What IS different is the envelope: the message wears the task
//! ([`crate::thread::TaskEnvelope`]) the way a hand-off wears its sender, and
//! carries the task as prose in its body so a harness that never learns the
//! field still reads the whole task.

use super::{edits, StoredAnswer, TaskWrite};
use crate::app::git::deferred::DeferredGitWork;
use crate::app::rpc::missing_param;
use crate::app::{AgentChoiceArgs, AppState};
use crate::thread::TaskEnvelope;
use crate::tracker::{
    Actor, Assignee, Task, TaskEventKind, DISPATCH_MOVES_FROM, IN_PROGRESS_STATUS,
};
use serde_json::{json, Value};

/// Where an `tasks.assign` was told to put the work.
///
/// One tagged value covering all five kinds, because assignment IS dispatch:
/// a second `dispatch` field beside the assignee would be two places for the
/// same decision to be made, and they could disagree. The two creating kinds
/// are not assignees — they resolve to [`Assignee::Agent`] before anything is
/// stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::app) enum AssignTarget {
    /// Nobody. Dispatches nothing and stops nothing already running.
    Nobody,
    User,
    ProjectAgent,
    Agent {
        agent_id: String,
    },
    NewWorkspace {
        name: Option<String>,
        isolation: Option<String>,
        choice: OwnedChoice,
        notify_user: Option<bool>,
        /// What to call the AGENT. Spelled `agent_name` on the wire because
        /// `name` on this kind is the workspace's, and one key meaning two
        /// things is how a caller names the wrong one.
        agent_name: Option<String>,
    },
    NewAgent {
        workspace_id: String,
        choice: OwnedChoice,
        notify_user: Option<bool>,
        /// The same key on both creating kinds, so a caller that learned it
        /// once has learned it.
        agent_name: Option<String>,
    },
}

/// What a new agent runs on, owned so it can travel to the deferred drain.
///
/// Every field is optional and an absent one is left OUT of the params
/// `agent.add` is called with: that verb reads the PRESENCE of a choice key to
/// tell "run it on this" from "run it on whatever the workspace runs on", and a
/// null would read as the former.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(in crate::app) struct OwnedChoice {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// What the agent is to be, for the device's own choice to answer.
    pub role: Option<String>,
    pub capability: Option<String>,
}

impl OwnedChoice {
    fn args(&self) -> AgentChoiceArgs<'_> {
        AgentChoiceArgs {
            harness: self.provider.as_deref(),
            model: self.model.as_deref(),
            effort: self.effort.as_deref(),
            role: self.role.as_deref(),
            capability: self.capability.as_deref(),
        }
    }

    /// Read off a wire assignee, which spells it `provider` — the word every
    /// wire verb uses. A TOOL spells the same thing `harness`, and maps it
    /// before it gets here.
    fn from_wire(value: &Value) -> OwnedChoice {
        let word = |key: &str| {
            value
                .get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|word| !word.is_empty())
                .map(str::to_string)
        };
        OwnedChoice {
            provider: word("provider"),
            model: word("model"),
            effort: word("effort"),
            role: word("role"),
            capability: word("capability"),
        }
    }
}

/// The agent's name off a creating assignee, checked here so a name that
/// cannot be had refuses before a checkout is cut for it.
fn agent_name_of(value: &Value) -> Result<Option<String>, String> {
    match value.get("agent_name").and_then(Value::as_str) {
        Some(word) => crate::agent::agent_name_from(word).map(Some),
        None => Ok(None),
    }
}

impl AssignTarget {
    /// Read an `assignee` off a verb's params. `null` is unassignment, which is
    /// a legible thing to ask for and not a missing param.
    pub(in crate::app) fn parse(value: Option<&Value>) -> Result<AssignTarget, String> {
        let Some(value) = value else {
            return Err(missing_param("assignee"));
        };
        if value.is_null() {
            return Ok(AssignTarget::Nobody);
        }
        let kind = value
            .get("kind")
            .and_then(Value::as_str)
            .ok_or("assignee: name a kind")?;
        let named = |key: &str| {
            value
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| format!("assignee {kind}: name a {key}"))
        };
        match kind {
            "user" => Ok(AssignTarget::User),
            "project_agent" => Ok(AssignTarget::ProjectAgent),
            "agent" => Ok(AssignTarget::Agent {
                agent_id: named("agent_id")?,
            }),
            "new_workspace" => Ok(AssignTarget::NewWorkspace {
                name: value
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                isolation: value
                    .get("isolation")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                choice: OwnedChoice::from_wire(value),
                notify_user: value.get("notify_user").and_then(Value::as_bool),
                agent_name: agent_name_of(value)?,
            }),
            "new_agent" => Ok(AssignTarget::NewAgent {
                workspace_id: named("workspace_id")?,
                choice: OwnedChoice::from_wire(value),
                notify_user: value.get("notify_user").and_then(Value::as_bool),
                agent_name: agent_name_of(value)?,
            }),
            other => Err(format!(
                "unknown assignee kind: {other} — one of user, project_agent, agent, \
                 new_workspace, new_agent"
            )),
        }
    }

    /// What is stored on the task for the kinds that name an assignee
    /// outright. The two creating kinds answer `None`: what they resolve to is
    /// only known once the agent exists.
    fn settled_assignee(&self) -> Option<Option<Assignee>> {
        match self {
            AssignTarget::Nobody => Some(None),
            AssignTarget::User => Some(Some(Assignee::User)),
            AssignTarget::ProjectAgent => Some(Some(Assignee::ProjectAgent)),
            AssignTarget::Agent { agent_id } => Some(Some(Assignee::Agent {
                agent_id: agent_id.clone(),
            })),
            _ => None,
        }
    }
}

/// One delivered task: where it went, and the receipt for the turn it queued.
pub(in crate::app) struct Delivered {
    pub workspace_id: Option<String>,
    pub entity_id: String,
    pub agent_id: String,
    pub operation_id: String,
}

impl Delivered {
    fn wire(&self, kind: &str) -> Value {
        json!({
            "kind": kind,
            "workspace_id": self.workspace_id,
            "entity_id": self.entity_id,
            "agent_id": self.agent_id,
            "operation_id": self.operation_id,
        })
    }
}

impl AppState {
    /// `tasks.assign` — hand a task to somebody, and start them on it.
    pub(crate) fn tasks_assign(&mut self, params: &Value) -> Result<Value, String> {
        let task_id = crate::app::require_str(params, "task_id")?;
        let (project_id, task) = self.tracker_task(&task_id)?;
        let target = AssignTarget::parse(params.get("assignee"))?;
        let note = crate::app::optional_nonempty_string(params, "note")?.map(str::to_string);
        self.assign_task_to(&project_id, task, target, note, Actor::User, None)
    }

    /// The whole of assignment, for the wire verb and for the MCP tool alike.
    ///
    /// `sender` is the agent that asked, when one did. It decides two things
    /// and nothing else: whose name is on the events, and whether the delivered
    /// message wears a sender beside its task.
    pub(in crate::app) fn assign_task_to(
        &mut self,
        project_id: &str,
        task: Task,
        target: AssignTarget,
        note: Option<String>,
        actor: Actor,
        sender: Option<crate::app::AgentSender<'_>>,
    ) -> Result<Value, String> {
        let now = crate::store::now_rfc3339();
        // The creating kinds cut a checkout, which is minutes of git on a big
        // repository. They hand that work to the drain and answer from there.
        if let AssignTarget::NewWorkspace {
            name,
            isolation,
            choice,
            notify_user,
            agent_name,
        } = &target
        {
            return self.dispatch_into_a_new_workspace(
                project_id,
                task,
                name,
                isolation,
                choice,
                notify_user.unwrap_or(matches!(actor, Actor::User)),
                agent_name.clone(),
                note,
                actor,
                sender,
            );
        }
        let mut write = TaskWrite::by(actor.clone(), task);
        let delivery = self.deliver_for(
            project_id,
            &write.task,
            &target,
            note.as_deref(),
            &actor,
            sender,
        )?;
        self.settle_assignment(&mut write, &target, &delivery, &actor, &now)?;
        let answered = self.commit_task_write(project_id, write, &now)?;
        Ok(json!({
            "task": answered["task"],
            "dispatch": delivery
                .as_ref()
                .map(|delivered| delivered.wire(target.wire_kind()))
                .unwrap_or(Value::Null),
        }))
    }

    /// Write the assignment down: who holds it, where the work went, and the
    /// column it moved to.
    fn settle_assignment(
        &mut self,
        write: &mut TaskWrite,
        target: &AssignTarget,
        delivery: &Option<Delivered>,
        actor: &Actor,
        now: &str,
    ) -> Result<(), String> {
        let settled = match target.settled_assignee() {
            Some(settled) => settled,
            // A creating kind resolves to the agent it made.
            None => delivery.as_ref().map(|delivered| Assignee::Agent {
                agent_id: delivered.agent_id.clone(),
            }),
        };
        write.task.assignee = settled.clone();
        // A task handed TO the user is one they are being asked about, so it
        // goes in their inbox whoever handed it over.
        if matches!(settled, Some(Assignee::User)) && write.task.set_watched(true) {
            write.event(actor, TaskEventKind::Watched, json!({}), now);
        }
        match &settled {
            None => write.event(actor, TaskEventKind::Unassigned, json!({}), now),
            Some(assignee) => write.event(
                actor,
                TaskEventKind::Assigned,
                json!({ "assignee": assignee }),
                now,
            ),
        }
        let Some(delivered) = delivery else {
            return Ok(());
        };
        // The agent that GOT the work is the one that most needs to hear about
        // the task, so the dispatch subscribes it. Not a courtesy: it is what
        // makes the hand-off two-way. `by: "assignment"` so a timeline reader
        // can tell this from an agent that asked.
        //
        // Read off the DELIVERY rather than off the stored assignee, because
        // `{kind:"project_agent"}` names no agent id and the project's agent
        // needs telling exactly as much as any other.
        if write.task.track(&delivered.agent_id).unwrap_or(false) {
            write.event(
                actor,
                TaskEventKind::Tracked,
                json!({ "agent_id": delivered.agent_id, "by": "assignment" }),
                now,
            );
        }
        // What the dispatch made is what the task is about now. These write no
        // `linked` events of their own: the `dispatched` event below already
        // says it, and two records of one fact read as two things happening.
        if let Some(workspace_id) = &delivered.workspace_id {
            crate::tracker::TaskLinks::add(&mut write.task.links.workspace_ids, workspace_id);
        }
        crate::tracker::TaskLinks::add(
            &mut write.task.links.conversation_ids,
            &delivered.entity_id,
        );
        write.event(
            actor,
            TaskEventKind::Dispatched,
            delivered.wire(target.wire_kind()),
            now,
        );
        // What the turn this dispatch just started is FOR. Its Complete moves
        // this task and no other; anything else the agent holds is a queue it
        // has not been asked about.
        self.dispatched_task
            .insert(delivered.agent_id.clone(), write.task.id.clone());
        // Starting work moves the card, but only off the columns that mean
        // "not started". A task already In progress, In review or Done was
        // put there deliberately, and a reassignment is not a reason to rewind
        // it.
        if write.task.is_open() && DISPATCH_MOVES_FROM.contains(&write.task.status.as_str()) {
            edits::move_to(
                write,
                IN_PROGRESS_STATUS,
                actor,
                json!({ "by": "dispatch" }),
                now,
            );
        }
        Ok(())
    }

    /// Put the task where the work will happen, for the kinds that need no
    /// checkout cut. `None` is `{kind:"user"}` and unassignment, which dispatch
    /// nothing.
    fn deliver_for(
        &mut self,
        project_id: &str,
        task: &Task,
        target: &AssignTarget,
        note: Option<&str>,
        actor: &Actor,
        sender: Option<crate::app::AgentSender<'_>>,
    ) -> Result<Option<Delivered>, String> {
        let (workspace_id, entity_id) = match target {
            AssignTarget::Nobody | AssignTarget::User => return Ok(None),
            AssignTarget::ProjectAgent => {
                let conversation =
                    self.project_ensure_conversation(&json!({ "project_id": project_id }))?;
                let entity_id = conversation["run_id"]
                    .as_str()
                    .ok_or("the project conversation has no owner")?
                    .to_string();
                (None, entity_id)
            }
            AssignTarget::Agent { agent_id } => {
                let entity_id = self.agent_of_this_project(project_id, agent_id)?;
                return self
                    .hand_over(task, &entity_id, agent_id, note, sender)
                    .map(Some);
            }
            AssignTarget::NewAgent {
                workspace_id,
                choice,
                notify_user,
                agent_name,
            } => {
                let added = self.add_agent_for_task(
                    project_id,
                    workspace_id,
                    choice.args(),
                    agent_name.as_deref(),
                    notify_user.unwrap_or(matches!(actor, Actor::User)),
                )?;
                return self
                    .hand_over(task, &added.1, &added.0, note, sender)
                    .map(Some);
            }
            // Answered by the drain; never reaches here.
            AssignTarget::NewWorkspace { .. } => return Ok(None),
        };
        let agent_id = self.ensure_primary_agent(&entity_id)?;
        let mut delivered = self.hand_over(task, &entity_id, &agent_id, note, sender)?;
        // The project's agent works in the repository itself, so there is no
        // workspace to name and the hand-off must not invent one.
        delivered.workspace_id = workspace_id;
        Ok(Some(delivered))
    }

    /// One agent of THIS project, or why it is none of this task's business.
    ///
    /// The same refusal the project-agent handlers raise, in the same words: a
    /// task reaches the agents of its own project and nothing else.
    fn agent_of_this_project(&self, project_id: &str, agent_id: &str) -> Result<String, String> {
        let entity_id = self
            .entity_of_agent(agent_id)
            .ok_or_else(|| format!("unknown agent_id: {agent_id}"))?;
        if self.projects.project_id_of(&entity_id) != Some(project_id) {
            return Err(format!("agent {agent_id} is not in project {project_id}"));
        }
        Ok(entity_id)
    }

    /// Put an agent on one of this project's workspaces, through the same
    /// `agent.add` the project surface's `add_workspace_agent` calls — minting
    /// the workspace's conversation owner first, because a workspace nobody has
    /// talked to has none and there would be nowhere for the agent to live.
    ///
    /// Answers `(agent_id, entity_id)`.
    fn add_agent_for_task(
        &mut self,
        project_id: &str,
        workspace_id: &str,
        choice: AgentChoiceArgs<'_>,
        agent_name: Option<&str>,
        notify_user: bool,
    ) -> Result<(String, String), String> {
        let workspace = self
            .workspaces
            .get(workspace_id)
            .cloned()
            .ok_or_else(|| format!("unknown workspace_id: {workspace_id}"))?;
        if workspace.project_id != project_id {
            return Err(format!(
                "workspace {workspace_id} is not in project {project_id}"
            ));
        }
        let mut params = choice.wire();
        params["workspace_id"] = json!(workspace_id);
        let conversation = self.workspace_ensure_conversation(&params)?;
        let entity_id = conversation["run_id"]
            .as_str()
            .ok_or("the workspace conversation has no owner")?
            .to_string();
        params["entity_id"] = json!(entity_id);
        // After the conversation is ensured: `name` is the workspace's to the
        // verbs above and the agent's to `agent.add`.
        if let Some(agent_name) = agent_name {
            params["name"] = json!(agent_name);
        }
        // The default is based on who assigned the task: an agent's new
        // worker is unwatched, while a worker the user creates is watched.
        // An explicit notify_user on the assignee overrides either default.
        params["made_by_agent"] = json!(true);
        params["notify_user"] = json!(notify_user);
        let added = self.agent_add(&params)?;
        let agent_id = added["agent"]["id"]
            .as_str()
            .ok_or("the added agent has no id")?
            .to_string();
        Ok((agent_id, entity_id))
    }

    /// Deliver the task into one agent's conversation.
    ///
    /// The post every other message goes through, so the turn is durable, gets
    /// a receipt, and starts the agent the way a reviewer message does. What is
    /// added is the envelope: the message wears the task, and carries it as
    /// prose so a harness that never learns the field still reads it.
    fn hand_over(
        &mut self,
        task: &Task,
        entity_id: &str,
        agent_id: &str,
        note: Option<&str>,
        sender: Option<crate::app::AgentSender<'_>>,
    ) -> Result<Delivered, String> {
        let operation_id = format!("op-{}", uuid::Uuid::new_v4());
        let posted =
            self.post_task_to_agent(task, entity_id, agent_id, note, &operation_id, sender)?;
        Ok(Delivered {
            // Where the agent is working, when it is working somewhere. All a
            // hand-off is given is a conversation, and a task that recorded
            // only that would name who is on it and not where the code is —
            // which is the question anybody reading the task later asks.
            // `None` for the project's agent, which works in no checkout.
            workspace_id: self.workspace_of_conversation(&posted),
            entity_id: posted,
            agent_id: agent_id.to_string(),
            operation_id,
        })
    }
}

/// What the assignment says: who handed over what, and anything they added.
///
/// A notice and not the task. Copying the task's text into the conversation
/// put a snapshot there that goes stale the moment anybody edits the task —
/// and that sits in the agent's context being compacted away before the work
/// even begins. So the agent is told what it has and reads it with `get_task`
/// when it is ready to start, which is also when the task is current.
pub(in crate::app) fn assignment_notice(task: &Task, assigner: &str, note: Option<&str>) -> String {
    let mut body = format!(
        "{assigner} assigned you Build task #{} — {}",
        task.number, task.title
    );
    if let Some(note) = note.map(str::trim).filter(|note| !note.is_empty()) {
        body.push_str("\n\n");
        body.push_str(note);
    }
    body
}

/// The task as a message wears it: enough to name it and link it.
///
/// No body, for the reason the notice has none — a card draws the number and
/// the title, and a copy of the text would be the same stale snapshot by
/// another route.
pub(in crate::app) fn envelope_of(task: &Task) -> TaskEnvelope {
    TaskEnvelope {
        task_id: task.id.clone(),
        number: task.number,
        title: task.title.clone(),
        links: task.links.clone(),
    }
}

impl AssignTarget {
    /// The word the `dispatched` event and the verb's answer call this kind.
    pub(in crate::app) fn wire_kind(&self) -> &'static str {
        match self {
            AssignTarget::Nobody => "none",
            AssignTarget::User => "user",
            AssignTarget::ProjectAgent => "project_agent",
            AssignTarget::Agent { .. } => "agent",
            AssignTarget::NewWorkspace { .. } => "new_workspace",
            AssignTarget::NewAgent { .. } => "new_agent",
        }
    }
}

impl AgentChoiceArgs<'_> {
    /// The choice as `agent.add` params, with an absent key left OUT rather
    /// than written as null — that verb reads the presence of a key.
    pub(in crate::app) fn wire(&self) -> Value {
        let mut params = json!({});
        for (key, value) in [
            ("provider", self.harness),
            ("model", self.model),
            ("effort", self.effort),
            ("role", self.role),
            ("capability", self.capability),
        ] {
            if let Some(value) = value {
                params[key] = json!(value);
            }
        }
        params
    }
}

// ------------------------------------------------- cutting a workspace ---

/// Everything the drain needs to finish a `new_workspace` dispatch once the
/// checkout exists.
///
/// Owned, and holding nothing: it travels to a blocking thread and comes back
/// to an `AppState` it cannot name while the git runs.
pub(in crate::app) struct DispatchPlan {
    project_id: String,
    task_id: String,
    workspace_id: String,
    choice: OwnedChoice,
    notify_user: bool,
    /// What to call the agent the drain will make, carried across the git.
    agent_name: Option<String>,
    note: Option<String>,
    actor: Actor,
    /// The agent that asked, as its two ids — [`crate::app::AgentSender`]
    /// borrows, and this has to outlive the call that built it.
    sender: Option<(String, String)>,
}

/// `workspace.create`'s own git, with the rest of the dispatch hung off the
/// end of it.
///
/// A wrapper rather than a second implementation: `run` and `invalidate` are
/// the workspace family's, untouched, so cutting a checkout for a task is
/// byte-for-byte the cut `workspace.create` makes. Only `settle` — which runs
/// with the mutex retaken and the checkout on disk — is this file's.
struct TaskDispatchWork {
    inner: Box<dyn DeferredGitWork>,
    plan: DispatchPlan,
}

impl DeferredGitWork for TaskDispatchWork {
    fn run(&self, params: &Value) -> Result<Value, String> {
        self.inner.run(params)
    }

    fn invalidates_on_error(&self) -> bool {
        self.inner.invalidates_on_error()
    }

    fn invalidate(&self, app: &mut AppState) {
        self.inner.invalidate(app);
    }

    /// The checkout is on disk; put an agent in it and hand it the task.
    ///
    /// The registry is reloaded here rather than left to `invalidate`, which
    /// runs AFTER this: until it is, the workspace the drain just cut is not
    /// one `workspace.ensure_conversation` can see, and the agent would have
    /// nowhere to live. Reloading twice is what `invalidate` already does to
    /// itself on every other path, and costs a directory read.
    fn settle(&self, app: &mut AppState, result: Value) -> Result<Value, String> {
        self.inner.settle(app, result)?;
        if let Err(error) = app.workspaces.reload() {
            return Err(format!("reload workspaces before dispatch: {error}"));
        }
        app.finish_new_workspace_dispatch(&self.plan)
    }
}

impl AppState {
    /// `{kind:"new_workspace"}` — cut a workspace, then put an agent in it and
    /// hand it the task.
    ///
    /// Cutting a checkout is seconds to minutes of git on a real repository, so
    /// it goes to the drain like every other checkout Build makes, and the rest
    /// of the dispatch goes with it. Nothing is written to the task here: a
    /// creation that fails must not leave a task assigned to an agent that
    /// was never made.
    #[allow(clippy::too_many_arguments)]
    fn dispatch_into_a_new_workspace(
        &mut self,
        project_id: &str,
        task: Task,
        name: &Option<String>,
        isolation: &Option<String>,
        choice: &OwnedChoice,
        notify_user: bool,
        agent_name: Option<String>,
        note: Option<String>,
        actor: Actor,
        sender: Option<crate::app::AgentSender<'_>>,
    ) -> Result<Value, String> {
        let mut params = json!({
            "project_id": project_id,
            "made_by_agent": matches!(actor, Actor::Agent { .. }),
            // The task's title when the caller named nothing: a workspace cut
            // for a task is about that task, and it names the branch too.
            "name": name.clone().unwrap_or_else(|| task.title.clone()),
        });
        if let Some(isolation) = isolation {
            params["isolation"] = json!(isolation);
        }
        let created = self.workspace_create(&params)?;
        let workspace_id = created["workspace_id"]
            .as_str()
            .ok_or("workspace.create answered no workspace_id")?
            .to_string();
        let plan = DispatchPlan {
            project_id: project_id.to_string(),
            task_id: task.id.clone(),
            workspace_id: workspace_id.clone(),
            choice: choice.clone(),
            notify_user,
            agent_name,
            note,
            actor,
            sender: sender
                .map(|sender| (sender.entity_id.to_string(), sender.agent_id.to_string())),
        };
        self.hang_dispatch_off_the_workspace_cut(plan)?;
        Ok(json!({ "task_id": task.id, "workspace_id": workspace_id, "pending": true }))
    }

    /// Take the git `workspace.create` just handed the drain and put the rest
    /// of the dispatch on the end of it.
    ///
    /// Wrapping what is already queued rather than queuing a second job: one
    /// deferred slot exists per dispatch, and a dispatch that queued its own
    /// would either overwrite the cut or race it.
    fn hang_dispatch_off_the_workspace_cut(&mut self, plan: DispatchPlan) -> Result<(), String> {
        let Some(crate::app::DeferredWork::Git(mut git)) = self.deferred_work.take() else {
            return Err(
                "workspace.create did not hand its checkout to the drain; cannot dispatch"
                    .to_string(),
            );
        };
        git.call = Box::new(TaskDispatchWork {
            inner: git.call,
            plan,
        });
        self.deferred_work = Some(crate::app::DeferredWork::Git(git));
        Ok(())
    }

    /// The drain's half: the checkout exists, so make the agent and deliver.
    fn finish_new_workspace_dispatch(&mut self, plan: &DispatchPlan) -> Result<Value, String> {
        // Re-read the task rather than carrying it through the git: the mutex
        // was free for the whole cut, and somebody may have moved it.
        let task = self
            .tracker_store()?
            .load_tracker_task(&plan.task_id)
            .stored()?
            .ok_or_else(|| format!("unknown task_id: {}", plan.task_id))?;
        let (agent_id, entity_id) = self.add_agent_for_task(
            &plan.project_id,
            &plan.workspace_id,
            plan.choice.args(),
            plan.agent_name.as_deref(),
            plan.notify_user,
        )?;
        let sender = plan
            .sender
            .as_ref()
            .map(|(entity_id, agent_id)| crate::app::AgentSender {
                entity_id,
                agent_id,
            });
        let mut delivered =
            self.hand_over(&task, &entity_id, &agent_id, plan.note.as_deref(), sender)?;
        delivered.workspace_id = Some(plan.workspace_id.clone());
        // Kept before the delivery is handed to the write: the answer names
        // the receipt for the turn that was actually queued, which is the
        // delivery's own and not the workspace cut's.
        let dispatch = delivered.wire("new_workspace");
        let now = crate::store::now_rfc3339();
        let mut write = TaskWrite::by(plan.actor.clone(), task);
        let target = AssignTarget::NewWorkspace {
            name: None,
            isolation: None,
            choice: plan.choice.clone(),
            notify_user: Some(plan.notify_user),
            agent_name: None,
        };
        self.settle_assignment(&mut write, &target, &Some(delivered), &plan.actor, &now)?;
        let answered = self.commit_task_write(&plan.project_id, write, &now)?;
        Ok(json!({ "task": answered["task"], "dispatch": dispatch }))
    }

    /// Who the notice says handed the work over.
    ///
    /// An agent is named the way its conversation is named — by the workspace
    /// it works in, or as the project's agent — rather than by its id, because
    /// the reader wants to know which of its colleagues is asking and an id is
    /// something to go and look up. It falls back to the id when Build cannot
    /// name the owner, which is the same fallback a client makes.
    ///
    /// `None` is the human, who the bridge knows no other name for.
    fn assigner_name(&self, sender: Option<crate::app::AgentSender<'_>>) -> String {
        let Some(sender) = sender else {
            return "The user".to_string();
        };
        let identity = self.agent_identity(sender.entity_id, sender.agent_id);
        match identity.owner {
            Some(owner) if owner.kind == crate::thread::AgentOwnerKind::Project => {
                format!("The {} project's agent", owner.name)
            }
            Some(owner) => format!("The {} agent", owner.name),
            None => format!("Agent {}", identity.id),
        }
    }

    /// Deliver a task into one conversation, with the envelope on it.
    fn post_task_to_agent(
        &mut self,
        task: &Task,
        entity_id: &str,
        agent_id: &str,
        note: Option<&str>,
        operation_id: &str,
        sender: Option<crate::app::AgentSender<'_>>,
    ) -> Result<String, String> {
        let assigner = self.assigner_name(sender);
        let requester = match sender {
            Some(sender) => Some(self.agent_requester(sender)?),
            None => None,
        };
        let posted = self.thread_post_handing_over_task(
            &json!({
                "entity_id": entity_id,
                "agent_id": agent_id,
                "body": assignment_notice(task, &assigner, note),
                "operation_id": operation_id,
            }),
            envelope_of(task),
            requester,
        )?;
        Ok(posted["entity_id"]
            .as_str()
            .unwrap_or(entity_id)
            .to_string())
    }
}
