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

use super::handoff::Handoff;
use super::{edits, StoredAnswer, TaskWrite};
use crate::app::rpc::missing_param;
use crate::app::{AgentChoiceArgs, AppState};
use crate::thread::TaskEnvelope;
use crate::tracker::{
    Actor, Assignee, Task, TaskEventKind, DISPATCH_MOVES_FROM, IN_PROGRESS_STATUS,
};
use serde_json::{json, Value};

/// Where an `tasks.assign` was told to put the work.
///
/// One tagged value covering all six kinds, because assignment IS dispatch:
/// a second `dispatch` field beside the assignee would be two places for the
/// same decision to be made, and they could disagree. The three creating
/// kinds are not assignees — they resolve to [`Assignee::Agent`] before
/// anything is stored.
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
        /// The same key on every creating kind, so a caller that learned it
        /// once has learned it.
        agent_name: Option<String>,
    },
    /// A new worker on the task's own project's conversation, working in the
    /// project's original sources (`tasks.assignProjectAgent`). The project
    /// is the task's; nothing the assignee carries can name another.
    NewProjectAgent {
        choice: OwnedChoice,
        notify_user: Option<bool>,
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
    pub(super) fn args(&self) -> AgentChoiceArgs<'_> {
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
            "new_project_agent" => Ok(AssignTarget::NewProjectAgent {
                choice: OwnedChoice::from_wire(value),
                notify_user: value.get("notify_user").and_then(Value::as_bool),
                agent_name: agent_name_of(value)?,
            }),
            other => Err(format!(
                "unknown assignee kind: {other} — one of user, project_agent, agent, \
                 new_workspace, new_agent, new_project_agent"
            )),
        }
    }

    /// What is stored on the task for the kinds that name an assignee
    /// outright. The two creating kinds answer `None`: what they resolve to is
    /// only known once the agent exists.
    pub(super) fn settled_assignee(&self) -> Option<Option<Assignee>> {
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
    pub(super) fn wire(&self, kind: &str) -> Value {
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
        let handoff = Handoff {
            note: crate::app::optional_nonempty_string(params, "note")?.map(str::to_string),
            operation_id: crate::app::conversations::optional_operation_id(params)?,
        };
        self.assign_task_to(&project_id, task, target, handoff, Actor::User, None)
    }

    /// The whole of assignment, for the wire verb and for the MCP tool alike.
    ///
    /// `sender` is the agent that asked, when one did. It decides two things
    /// and nothing else: whose name is on the events, and whether the delivered
    /// message wears a sender beside its task.
    ///
    /// A hand-off naming an `operation_id` this task has already dispatched
    /// answers that dispatch again and does nothing else, so a retried
    /// request neither delivers twice nor makes a second agent.
    pub(in crate::app) fn assign_task_to(
        &mut self,
        project_id: &str,
        task: Task,
        target: AssignTarget,
        handoff: Handoff,
        actor: Actor,
        sender: Option<crate::app::AgentSender<'_>>,
    ) -> Result<Value, String> {
        if !matches!(target, AssignTarget::Nobody | AssignTarget::User) {
            return self.accept_task_delivery(project_id, task, target, handoff, actor, sender);
        }
        let now = crate::store::now_rfc3339();
        let mut write = TaskWrite::by(actor.clone(), task);
        self.settle_assignment(&mut write, &target, &None, &actor, &now)?;
        let answered = self.commit_task_write(project_id, write, &now)?;
        Ok(json!({"task": answered["task"], "dispatch": Value::Null}))
    }

    /// Write the assignment down: who holds it, where the work went, and the
    /// column it moved to.
    pub(super) fn settle_assignment(
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

    /// One agent of THIS project, or why it is none of this task's business.
    ///
    /// The same refusal the project-agent handlers raise, in the same words: a
    /// task reaches the agents of its own project and nothing else.
    pub(in crate::app) fn agent_of_this_project(
        &self,
        project_id: &str,
        agent_id: &str,
    ) -> Result<String, String> {
        let entity_id = self
            .entity_of_agent(agent_id)
            .ok_or_else(|| format!("unknown agent_id: {agent_id}"))?;
        if self.projects.project_id_of(&entity_id) != Some(project_id) {
            return Err(format!("agent {agent_id} is not in project {project_id}"));
        }
        Ok(entity_id)
    }

    /// Review publication supplies its durable operation ID. Reuse the usual
    /// task hand-off and assignment bookkeeping without moving its In review card.
    pub(in crate::app) fn dispatch_review_reviewer(
        &mut self,
        project_id: &str,
        published: &Task,
        note: &str,
        operation_id: &str,
        actor: &Actor,
        sender: Option<&crate::app::mcp::McpConversationGeneration>,
    ) -> Result<(), String> {
        let (_, task) = self.tracker_task(&published.id)?;
        let timeline = self
            .tracker_store()?
            .load_tracker_timeline(&task.id)
            .stored()?;
        if timeline.iter().any(|entry| {
            matches!(entry,
                crate::tracker::TimelineEntry::Event(event)
                    if event.kind == TaskEventKind::Dispatched
                        && event.payload["operation_id"] == operation_id
            )
        }) {
            return Ok(());
        }
        if matches!(published.assignee, None | Some(Assignee::User)) {
            return Ok(());
        }
        // Publication survives a creator reset; a new reviewer hand-off must
        // still belong to the conversation generation that admitted the open.
        if let Some(sender) = sender {
            sender.guard(self)?;
        }
        let target = match &published.assignee {
            None | Some(Assignee::User) => return Ok(()),
            Some(Assignee::ProjectAgent) => AssignTarget::ProjectAgent,
            Some(Assignee::Agent { agent_id }) => AssignTarget::Agent {
                agent_id: agent_id.clone(),
            },
        };
        let sender = sender.map(|sender| crate::app::AgentSender {
            entity_id: &sender.entity_id,
            agent_id: &sender.agent_id,
        });
        self.accept_task_delivery(
            project_id,
            task,
            target,
            Handoff {
                note: Some(note.into()),
                operation_id: Some(operation_id.into()),
            },
            actor.clone(),
            sender,
        )?;
        Ok(())
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
            AssignTarget::NewProjectAgent { .. } => "new_project_agent",
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

impl AppState {
    /// Who the notice says handed the work over.
    ///
    /// An agent is named the way its conversation is named — by the workspace
    /// it works in, or as the project's agent — rather than by its id, because
    /// the reader wants to know which of its colleagues is asking and an id is
    /// something to go and look up. It falls back to the id when Build cannot
    /// name the owner, which is the same fallback a client makes.
    ///
    /// `None` is the human, who the bridge knows no other name for.
    pub(super) fn assigner_name(&self, sender: Option<crate::app::AgentSender<'_>>) -> String {
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
}
