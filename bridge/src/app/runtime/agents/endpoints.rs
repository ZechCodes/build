use crate::app::runtime::terminals::terminal_scope_root;
use crate::app::{
    agent_tab_id, attach_to_tab, attach_view, optional_nonempty_string, require_str,
    working_time_json, AddressedSession, AppState, DeliveryRunner, LifecycleDiagnostic,
    PendingAgentTurn, SelfReport, TabFacts, TabKey, TurnText, NEW_THREAD_MESSAGES_PROMPT,
};
#[cfg(test)]
use crate::app::{Tab, TabRole};
use crate::carrier::SessionSender;
use crate::harness::harness_for;
use crate::harness::installed::held_refusal;
use crate::harness::AgentStatus;
use crate::models::{AgentProvider, ModelChoice};
use crate::reaper::Retirement;
use crate::screen::ScreenHandle;
use crate::store::now_rfc3339;
use crate::thread::SessionInstance;
use crate::timing::FrameTimer;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

/// The most of an opening message a bubble's title wears.
const TITLE_MAX_CHARS: usize = 120;

/// The agent a verb's parameters name, resolved whole.
///
/// Five readings that only make sense together and are only ever taken
/// together: which entity, which of its agents, the checkout that agent works
/// in, the harness it runs, and whether its thread holds anything it has not
/// been told about. A verb that has one of these has everything it needs to
/// queue a turn and to answer.
pub(in crate::app) struct AddressedAgent {
    pub(in crate::app) entity_id: String,
    pub(in crate::app) agent_id: String,
    pub(in crate::app) conversation_id: String,
    pub(in crate::app) root: std::path::PathBuf,
    pub(in crate::app) model_choice: ModelChoice,
    pub(in crate::app) choice_revision: u64,
    pub(in crate::app) has_unread: bool,
}

pub(in crate::app) struct AgentSpawnRequest<'a> {
    pub(in crate::app) owner: &'a str,
    pub(in crate::app) agent_id: &'a str,
    pub(in crate::app) conversation_id: &'a str,
    pub(in crate::app) model_choice: &'a ModelChoice,
    pub(in crate::app) force_fresh: bool,
    pub(in crate::app) phase: &'a str,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::app) enum DigestScope {
    List,
    Detail,
}

/// Attach this client to the agent of a WORKTREE, addressed the way the
/// calling surface already knows it.
///
/// The same attach as `term.attach`, but by what a surface holds rather than by
/// a wire id it cannot compute: `id` for a surface that is a plan or a run, and
/// otherwise the scope shapes `term.create`/`term.list` take (`run_id`,
/// `project_id`+`worktree_id`, `project_id` for the primary checkout). Both
/// resolve server-side to the same canonical root — the tab registry's key — so
/// a run and the directory it works in reach one agent, not two.
///
/// The reply carries `provider` — the harness the tab runs, or null where no
/// agent has ever run — because a dead agent's retained screen is the only
/// record of which harness painted it, and the tab's start-again offer leads
/// with that one.
///
/// **Never errors because no agent is running** — `live: false` with the last
/// (or a blank) snapshot is the contract, because a tab must still show what
/// its agent did before it died, and because the Agent tab is a fixture on
/// every worktree surface: mounting it must not spawn anything. An unknown
/// entity or scope errors, and so does an entity with no worktree (an approved
/// or abandoned plan): its disposable worktree is gone, so there is no worktree
/// to host an agent and the surface renders its empty state instead.
///
/// It errors for an agent whose session has no terminal ([`no_terminal_here`]).
/// A client that reads `has_terminal` never asks, and one that predates the
/// field gets a sentence rather than a blank grid it will sit in forever.
pub(in crate::app) fn agent_attach(
    state: &Arc<Mutex<AppState>>,
    sender: &SessionSender,
    params: &Value,
    timer: &FrameTimer,
) -> Result<Value, String> {
    // Grid defaults = the orchestrator's agent PTY size (40 rows × 120 cols).
    let cols = params.get("cols").and_then(Value::as_u64).unwrap_or(120) as u16;
    let rows = params.get("rows").and_then(Value::as_u64).unwrap_or(40) as u16;

    let requested_agent = named_agent_id(params)?;

    let mut guard = timer.lock(state);
    let s = &mut *guard;
    // The id is opaque (plan-… / run-…); what it resolves to is a worktree,
    // because that is what an agent works in. Without one, the scope params
    // resolve to the same thing — never a client-supplied path (spec §1).
    let entity_id = params
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string);
    let root = match &entity_id {
        Some(entity_id) => s.entity_agent_root(entity_id)?,
        None => terminal_scope_root(s, params)?,
    };
    // Which agent: the one the rail named, or the entity's first — so a
    // surface that predates the rail still attaches to the agent it always did.
    // A scope-addressed attach with no entity can only mean the agent already
    // running there.
    let agent_id = match (&entity_id, requested_agent.as_deref()) {
        (Some(entity_id), requested) => s.resolve_agent(entity_id, requested)?.id,
        (None, Some(requested)) => requested.to_string(),
        // A worktree Build owns nothing in yet — an unadopted checkout, the
        // primary one — has no agent to name, so the screen a client mounts
        // there is addressed by the worktree itself until one is born.
        (None, None) => s
            .session_registry
            .first_agent_id_at(&root)
            .unwrap_or_else(|| crate::worktree::external_worktree_id(&root)),
    };
    if let Some(expected) = optional_nonempty_string(params, "conversation_id")? {
        let entity_id = entity_id
            .as_deref()
            .ok_or("conversation_id requires an entity id")?;
        let actual = s
            .resolve_conversation_address(entity_id, Some(&agent_id))?
            .conversation_id;
        if expected != actual {
            return Err(format!(
                "stale conversation_id {expected}; agent {agent_id} is bound to {actual}"
            ));
        }
    }
    let key = TabKey::agent(&root, &agent_id);
    if !s.session_registry.contains(&key) {
        // No agent has run here yet: a blank, dead screen, and the tab opens on
        // the first delivery. The client still registers — on the screen this
        // worktree's agent will be born onto — because it must go live where it
        // stands when that delivery comes, not sit blank until the human
        // unmounts and remounts the tab.
        //
        // The handle is cloned under the lock and registered on with it
        // released, so a spawn can carry this screen's clients away in between;
        // the register follows them, because a carried screen points at the one
        // its clients went to.
        let term_id = agent_tab_id(&agent_id);
        let screen =
            s.session_registry
                .waiting_screen_for_attach(key.clone(), &term_id, cols, rows);
        drop(guard);
        let reading = screen.attach(sender, Some((cols, rows)));
        // A screen with no session behind it is dead by definition, and names
        // no harness: nothing has ever run here to name one.
        return Ok(attach_view(
            TabFacts {
                term_id,
                live: false,
                provider: None,
            },
            reading,
        ));
    }
    let attachment = s.attachment(&key)?;
    drop(guard);
    Ok(attach_to_tab(attachment, sender, cols, rows))
}

/// Open a worktree's agent with nothing to say to it — the surface's "Start
/// agent" button, and the "Restart" the human needs when the harness exits on
/// its own (codex running a self-update and quitting, claude crashing).
///
/// Every other way to get an agent is a turn: you say something and the agent
/// is spawned to hear it. That leaves no way to simply have one running, and no
/// way back after an exit short of inventing a message. This verb is that way,
/// and it is the only spawn with no prompt behind it — which is why the turn it
/// queues carries no text.
///
/// It takes the same queue every other turn does, so the reply is the entity's
/// state and never the harness's: the tab id it answers with is the one the
/// agent's own identity mints, reserved here and filled in when the session
/// opens. The owner must be an entity that owns a worktree — `.build/mcp.json`
/// routes `done` per owner, so an agent with nobody to report to is worse than
/// none.
pub(in crate::app) fn agent_start(
    state: &Arc<Mutex<AppState>>,
    params: &Value,
    timer: &FrameTimer,
) -> Result<Value, String> {
    let agent = {
        let mut s = timer.lock(state);
        let requested_entity = params
            .get("id")
            .or_else(|| params.get("run_id"))
            .or_else(|| params.get("plan_id"))
            .and_then(Value::as_str);
        if requested_entity.is_some_and(|entity_id| s.plans.contains_key(entity_id)) {
            return Err(crate::app::tasks::TASKS_RETIRED_ERROR.to_string());
        }
        let agent = s.addressed_agent(params)?;
        s.delivery_queue.enqueue(PendingAgentTurn {
            operation_id: None,
            root: agent.root.clone(),
            owner: agent.entity_id.clone(),
            agent_id: agent.agent_id.clone(),
            conversation_id: agent.conversation_id.clone(),
            model_choice: agent.model_choice.clone(),
            choice_revision: agent.choice_revision,
            interrupt: false,
            // The button means "give me an agent", not "go do something" — so a
            // start with nothing waiting says nothing, and the human drives from
            // there. But the reviewer's words are durable on the thread and an
            // agent only learns of them by being TOLD to call
            // `read_unread_messages`; a fresh harness has no reason to.
            // Restarting after a crash with messages outstanding would silently
            // ignore every one of them. A hand-started agent has no context, so
            // what waits for it gets the cold form: the conversation protocol
            // and the catch-up packet around the nudge.
            say: agent.has_unread.then(|| TurnText {
                cold: crate::orchestrator::conversation_prompt(NEW_THREAD_MESSAGES_PROMPT),
                warm: NEW_THREAD_MESSAGES_PROMPT.to_string(),
            }),
            phase: "start",
            wants_catch_up: true,
            survives_refusal: false,
        });
        s.touch_attention(&agent.entity_id);
        agent
    };
    DeliveryRunner::drain(state, timer);

    Ok(json!({
        "term_id": agent_tab_id(&agent.agent_id),
        "agent_id": agent.agent_id,
        "notified": agent.has_unread,
    }))
}

/// Stop the turn running in one exact agent session without ending that
/// session or posting anything to its conversation.
pub(in crate::app) fn agent_interrupt(
    state: &Arc<Mutex<AppState>>,
    params: &Value,
    timer: &FrameTimer,
) -> Result<Value, String> {
    let entity_id = require_str(params, "entity_id")?;
    let agent_id = require_str(params, "agent_id")?;
    let conversation_id = require_str(params, "conversation_id")?;
    let session = {
        let s = timer.lock(state);
        if s.plans.contains_key(&entity_id) {
            return Err(crate::app::tasks::TASKS_RETIRED_ERROR.to_string());
        }
        let address = s.resolve_conversation_address(&entity_id, Some(&agent_id))?;
        if address.conversation_id != conversation_id {
            return Err(format!(
                "agent.interrupt: stale conversation_id {conversation_id}; agent {agent_id} is bound to {}",
                address.conversation_id
            ));
        }
        let root = s.entity_agent_root(&entity_id)?;
        let tab = s
            .session_registry
            .agent_snapshot(&TabKey::agent(&root, &agent_id))
            .ok_or_else(|| "agent.interrupt: this agent has no running session".to_string())?;
        let exact = tab.role.agent() == Some((entity_id.as_str(), agent_id.as_str()))
            && tab.instance.as_ref().is_some_and(|instance| {
                instance.entity_id == entity_id
                    && instance.agent_id == agent_id
                    && instance.conversation_id == conversation_id
                    && instance.checkout == root.display().to_string()
            });
        if !exact {
            return Err(
                "agent.interrupt: the running session does not match this conversation".to_string(),
            );
        }
        if !tab.live || matches!(tab.session.status(), AgentStatus::Ended { .. }) {
            return Err("agent.interrupt: this agent has no running session".to_string());
        }
        if !matches!(tab.session.status(), AgentStatus::Working) {
            return Err("agent.interrupt: this agent is not running a turn".to_string());
        }
        if !tab.session.can_interrupt() {
            return Err("agent.interrupt: this running turn cannot be interrupted".to_string());
        }
        Arc::clone(&tab.session)
    };
    session
        .interrupt()
        .map_err(|error| format!("agent.interrupt: {error}"))?;
    Ok(json!({
        "entity_id": entity_id,
        "agent_id": agent_id,
        "conversation_id": conversation_id,
        "interrupted": true,
    }))
}

/// The conversation event one reported activity becomes. The five kinds are the
/// same five, named once here so the mapping cannot drift.
pub(in crate::app) fn activity_event_kind(
    activity: &crate::harness::AgentActivity,
) -> crate::thread::ThreadEventKind {
    use crate::harness::AgentActivity;
    use crate::thread::ThreadEventKind;
    match activity {
        AgentActivity::Compaction { .. } => ThreadEventKind::Compaction,
        AgentActivity::Reasoning { .. } => ThreadEventKind::Reasoning,
        AgentActivity::ToolUse { .. } => ThreadEventKind::ToolUse,
        AgentActivity::ToolResult { .. } => ThreadEventKind::ToolResult,
        AgentActivity::Narration { .. } => ThreadEventKind::Narration,
        AgentActivity::TaskUpdate { .. } => ThreadEventKind::TaskUpdate,
    }
}

/// Whether a tab holds an agent that is working right now.
///
/// Three things have to be true, and each rules out a different lie: the tab
/// is an agent's (a shell is the human's own hands, however busy it looks), its
/// stream is still open (a dead agent's retained screen is not a heartbeat),
/// and the session itself reports [`AgentStatus::Working`].
///
/// That last one used to be the age of the last paint, read straight off the
/// PTY. It is now the session's own answer, because the paint clock is a guess
/// only a terminal is forced to make — a harness that knows when its turn began
/// and ended has a better one, and must be able to give it. For a PTY the guess
/// is unchanged: [`crate::pty::PtySession`] synthesizes `Working` from exactly
/// the two conjuncts that moved, so this reports what it always has.
#[cfg(test)]
pub(in crate::app) fn agent_is_working(tab: &Tab) -> bool {
    matches!(tab.role, TabRole::Agent { .. })
        && tab.live
        && matches!(tab.session.status(), AgentStatus::Working)
}

/// A set of words as a sentence says them: "a, b or c".
pub(in crate::app) fn listed<const N: usize>(words: [&str; N]) -> String {
    match words.split_last() {
        None => String::new(),
        Some((last, [])) => format!("{last:?}"),
        Some((last, rest)) => format!(
            "{} or {last:?}",
            rest.iter()
                .map(|word| format!("{word:?}"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Whether the user should see this conversation in their inbox.
///
/// `notify_user` says so outright. Absent, the answer is whoever asked: the
/// UI's own creates carry no agent caller and are the user's own doing, and an
/// agent's do.
fn watching_asked(params: &Value) -> bool {
    if let Some(asked) = params.get("notify_user").and_then(Value::as_bool) {
        return asked;
    }
    params.get("made_by_agent").and_then(Value::as_bool) != Some(true)
}

pub(in crate::app) fn has_agent_choice(params: &Value) -> bool {
    ["provider", "model", "effort"]
        .iter()
        .any(|key| params.get(key).is_some())
}

/// Parse and validate the optional provider/model/effort params of a request,
/// falling back to `default` — the account's default harness — when the caller
/// names no provider.
///
/// This is the one place a wire provider becomes a persisted one. Every token
/// names one harness concretely, so what this mints is what the agent is locked
/// to: nothing is ever resolved a second time.
pub(in crate::app) fn model_choice_from(
    params: &Value,
    default: AgentProvider,
) -> Result<ModelChoice, String> {
    model_choice_over(
        params,
        ModelChoice {
            provider: default,
            model: None,
            effort: None,
        },
    )
}

/// The same parse, over a choice something else already resolved — the
/// device's grid for this (task, scope), or the entity's own selection.
///
/// Field by field, so a caller that names only an effort keeps the model the
/// grid chose. What is named wins; what is not named is inherited; and a
/// caller that names nothing gets `beneath` unchanged.
pub(in crate::app) fn model_choice_over(
    params: &Value,
    beneath: ModelChoice,
) -> Result<ModelChoice, String> {
    let named = |key: &str| {
        params
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|word| !word.is_empty())
            .map(str::to_string)
    };
    let provider = match params.get("provider").and_then(Value::as_str) {
        // No preference means what was resolved beneath; a named one means
        // itself.
        None | Some("") => beneath.provider,
        Some(named) => AgentProvider::from_wire(named)
            .ok_or_else(|| format!("unknown agent provider: {named}"))?,
    };
    let choice = ModelChoice {
        provider,
        model: named("model").or(beneath.model),
        effort: named("effort").or(beneath.effort),
    };
    choice.validate()?;
    Ok(choice)
}

/// The optional `agent_id` a verb was addressed to. Only omission or null means
/// the entity's primary; an explicitly empty or malformed identity is refused
/// before it can broaden into somebody else's conversation.
pub(in crate::app) fn named_agent_id(params: &Value) -> Result<Option<String>, String> {
    optional_nonempty_string(params, "agent_id").map(|id| id.map(str::to_string))
}

impl AppState {
    /// The canonical checkout an entity's agents work in — the key they are
    /// registered under. A run's is its worktree; a task's is the project's
    /// primary checkout, because task agents never get a worktree.
    ///
    /// A task whose workspace is gone (approved, abandoned) has no agent;
    /// that is a refusal, not a blank tab, because there is nothing for an
    /// agent to run in.
    pub(in crate::app) fn entity_agent_root(
        &self,
        entity_id: &str,
    ) -> Result<std::path::PathBuf, String> {
        if let Some(plan) = self.plans.get(entity_id) {
            return plan
                .workspace
                .as_ref()
                .map(|workspace| Self::canonical_root(&workspace.checkout))
                .ok_or_else(|| "the task has no session, so it has no agent".to_string());
        }
        if let Some(run) = self.runs.get(entity_id) {
            return Ok(Self::canonical_root(&run.worktree.path));
        }
        Err("unknown id".to_string())
    }

    /// An entity's agents, whichever kind of entity it is.
    pub(in crate::app) fn entity_agents(
        &self,
        entity_id: &str,
    ) -> Result<&crate::agent::AgentRoster, String> {
        if let Some(plan) = self.plans.get(entity_id) {
            return Ok(&plan.agents);
        }
        if let Some(run) = self.runs.get(entity_id) {
            return Ok(&run.agents);
        }
        Err("unknown id".to_string())
    }

    /// The agent a verb means — the one it named, or the entity's first, so
    /// every verb that predates agents keeps addressing the agent it always
    /// did.
    pub(in crate::app) fn resolve_agent(
        &self,
        entity_id: &str,
        agent_id: Option<&str>,
    ) -> Result<crate::agent::Agent, String> {
        self.entity_agents(entity_id)?.resolve(agent_id).cloned()
    }

    /// What an authenticated agent id turns out to be: a coding session working
    /// an entity, or a router session deciding a capture. Nothing else can
    /// reach the control socket, and the two get different tools.
    pub(in crate::app) fn addressed_session(&self, agent_id: String) -> Option<AddressedSession> {
        if crate::router::is_router_agent(&agent_id) {
            return self.capture_of_router_agent(&agent_id).map(|capture_id| {
                AddressedSession::Router {
                    capture_id,
                    agent_id,
                }
            });
        }
        self.entity_of_agent(&agent_id)
            .map(|entity_id| AddressedSession::Coding {
                entity_id,
                agent_id,
            })
    }

    /// Which entity owns an agent id. The MCP control plane authenticates an
    /// AGENT (the harness knows its own id and its own token); the lifecycle it
    /// drives belongs to the entity behind it.
    pub(in crate::app) fn entity_of_agent(&self, agent_id: &str) -> Option<String> {
        self.plans
            .iter()
            .map(|(id, plan)| (id, &plan.agents))
            .chain(self.runs.iter().map(|(id, run)| (id, &run.agents)))
            .find(|(_, roster)| roster.by_id(agent_id).is_some())
            .map(|(id, _)| id.clone())
    }

    /// The agent the system delivers to on `entity_id`, minting one when the
    /// human has left the branch with none.
    ///
    /// The door every path that MUST be heard takes — a post, a start, a
    /// routed instruction — so none of them reaches for a roster that may be
    /// empty. The agent is created on the entity's own persisted choice, which
    /// is the account's default harness unless somebody named another one when
    /// the entity was created or adopted.
    ///
    /// A task always holds its one agent, so only a branch is ever minted on.
    pub(in crate::app) fn ensure_primary_agent(
        &mut self,
        entity_id: &str,
    ) -> Result<String, String> {
        self.ensure_primary_agent_on(entity_id, None)
    }

    /// The same door, for the caller that has already taken a choice off the
    /// request it is serving: a provider card pressed on an empty branch names
    /// what the agent about to be created runs, and that answer is newer than
    /// anything persisted. Every other caller passes `None` and the mint reads
    /// [`AppState::mint_model_choice`].
    fn ensure_primary_agent_on(
        &mut self,
        entity_id: &str,
        asked: Option<ModelChoice>,
    ) -> Result<String, String> {
        if let Some(primary) = self.entity_agents(entity_id)?.primary() {
            return Ok(primary.id.clone());
        }
        let choice = match asked {
            Some(asked) => asked,
            None => self.mint_model_choice(entity_id)?,
        };
        let owner = self.agent_owner(entity_id);
        let mut active = self.take_run(entity_id)?;
        let agent_id = active
            .agents
            .ensure_primary(owner, choice, &now_rfc3339())
            .id
            .clone();
        self.finish_run_mutation(entity_id.to_string(), active)?;
        Ok(agent_id)
    }

    /// Resolve the agent a verb's parameters address, minting one where asking
    /// for an agent is what the verb means.
    ///
    /// The order is the rule. The provider picker rides the parameters, so it
    /// is parsed and written FIRST: an unrunnable provider refuses before
    /// anything has been opened, and the agent resolved after it is resolved on
    /// the choice the human just made. Then the entity's own agent — named, or
    /// the primary, which a branch the human emptied is given — then the
    /// checkout it works in and the harness it runs, which is the AGENT's and
    /// not the entity's: the several agents on a branch need not share one.
    pub(in crate::app) fn addressed_agent(
        &mut self,
        params: &Value,
    ) -> Result<AddressedAgent, String> {
        // `run_id` is the adopting caller's spelling: a worktree surface with no
        // run yet mints one and forwards the verb, and that helper names the id
        // it just minted. Same entity either way.
        let entity_id = params
            .get("id")
            .or_else(|| params.get("run_id"))
            .or_else(|| params.get("plan_id"))
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or("missing id")?
            .to_string();
        let named = named_agent_id(params)?;
        let roster_is_empty = self.entity_agents(&entity_id)?.is_empty();
        let expected_conversation = optional_nonempty_string(params, "conversation_id")?;
        if roster_is_empty && named.is_some() {
            self.resolve_agent(&entity_id, named.as_deref())?;
        }
        if roster_is_empty && expected_conversation.is_some() {
            return Err("no conversation exists for the expected conversation_id".to_string());
        }
        // A provider card on an empty branch seeds the one agent it is about to
        // create. Once an agent exists, settings are written only on that exact
        // agent and its provider is its durable harness identity.
        let asked = if roster_is_empty && has_agent_choice(params) {
            let chosen = model_choice_from(params, self.mint_model_choice(&entity_id)?.provider)?;
            self.set_entity_model_choice(&entity_id, chosen.clone())?;
            Some(chosen)
        } else {
            None
        };
        let agent_id = match named.as_deref() {
            None if roster_is_empty => self.ensure_primary_agent_on(&entity_id, asked)?,
            named => self.resolve_agent(&entity_id, named)?.id,
        };
        if let Some(expected) = expected_conversation {
            let actual = self
                .entity_agents(&entity_id)?
                .resolve(Some(&agent_id))?
                .conversation_id();
            if expected != actual {
                return Err(format!(
                    "stale conversation_id {expected}; agent {agent_id} is bound to {actual}"
                ));
            }
        }
        if !roster_is_empty && has_agent_choice(params) {
            let locked = self
                .entity_agents(&entity_id)?
                .resolve(Some(&agent_id))?
                .choice
                .provider;
            let chosen = model_choice_from(params, locked)?;
            if chosen.provider != locked {
                return Err(format!(
                    "agent.start: the agent is locked to {}",
                    locked.label()
                ));
            }
            self.set_agent_model_choice(&entity_id, &agent_id, chosen)?;
        }
        let root = self.entity_agent_root(&entity_id)?;
        let has_unread = self
            .agent_conversation(&entity_id, Some(&agent_id))?
            .has_unread();
        let roster = self.entity_agents(&entity_id)?;
        let agent = roster
            .by_id(&agent_id)
            .expect("the agent was just resolved on this roster");
        Ok(AddressedAgent {
            has_unread,
            model_choice: agent.choice.clone(),
            choice_revision: agent.choice_revision,
            conversation_id: agent.conversation_id().to_string(),
            entity_id,
            agent_id,
            root,
        })
    }

    /// The agent an entity dispatches with. A start with no turn behind it still
    /// has to honor the provider/model the human chose for this worktree — the
    /// sheet's answer, or the run's own — rather than silently defaulting.
    pub(in crate::app) fn entity_model_choice(
        &self,
        entity_id: &str,
    ) -> Result<ModelChoice, String> {
        if let Some(plan) = self.plans.get(entity_id) {
            return Ok(plan.model_choice.clone());
        }
        if let Some(run) = self.runs.get(entity_id) {
            return Ok(run.model_choice.clone());
        }
        Err("unknown id".to_string())
    }

    /// Whether any of an entity's agents currently holds a running harness
    /// process. An entity with no worktree has no agent and therefore none
    /// running.
    pub(in crate::app) fn entity_agent_is_live(&self, entity_id: &str) -> bool {
        let Ok(root) = self.entity_agent_root(entity_id) else {
            return false;
        };
        let Ok(roster) = self.entity_agents(entity_id) else {
            return false;
        };
        roster
            .iter()
            .any(|agent| self.agent_is_live(&root, &agent.id))
    }

    /// Whether one agent's harness process is running right now.
    pub(in crate::app) fn agent_is_live(&self, root: &std::path::Path, agent_id: &str) -> bool {
        self.session_registry
            .agent_is_live(&TabKey::agent(root, agent_id))
    }

    /// Point an entity's agents at a different provider/model. The persisted
    /// choice is what every later start and turn reads, so a switch made at
    /// the Agent tab has to outlive both this process and this daemon.
    ///
    /// A running harness cannot be re-provisioned under itself: the process
    /// would keep the old provider while the record claimed the new one, and
    /// the human would have no owner-side handle on what is actually running.
    /// So a PROVIDER move is refused while a session is live. Re-asserting the
    /// choice the entity already has is not a move and passes through, which is
    /// what keeps a start that always names its provider idempotent — and a
    /// model or effort edit is not a move either: it is what the next start
    /// spends, so it persists and waits for one.
    pub(in crate::app) fn set_entity_model_choice(
        &mut self,
        entity_id: &str,
        choice: ModelChoice,
    ) -> Result<(), String> {
        if self.entity_model_choice(entity_id)? == choice {
            return Ok(());
        }
        // Only a PROVIDER move is refused while a session runs: that is the one
        // that would leave the process on the old harness while the record
        // claimed the new one. A model or effort edit is what the next start
        // spends, so it persists under a live session and waits for it.
        if self.entity_model_choice(entity_id)?.provider != choice.provider
            && self.entity_agent_is_live(entity_id)
        {
            return Err(format!(
                "agent.start: an agent session is already running on {} — stop the current \
                 session first, then start it on {}",
                self.entity_model_choice(entity_id)?.provider.label(),
                choice.provider.label()
            ));
        }
        if self.plans.contains_key(entity_id) {
            let mut active = self.take_plan(entity_id)?;
            active.model_choice = choice;
            let persisted = self.persist_plan_record(entity_id, &active);
            self.plans.insert(entity_id.to_string(), active);
            self.note_entity_changed(entity_id);
            return persisted;
        }
        let mut active = self.take_run(entity_id)?;
        active.model_choice = choice;
        let persisted = self.persist_run_record(entity_id, &active);
        self.runs.insert(entity_id.to_string(), active);
        self.note_entity_changed(entity_id);
        persisted
    }

    /// Kill, reap, and forget EVERY agent rooted in a worktree, telling every
    /// attached client the tabs are gone.
    ///
    /// An agent whose owner no longer exists is worse than no agent: it keeps
    /// working and reports `done` into the unknown-entity log forever. So the
    /// two verbs that remove a run while KEEPING its worktree — release
    /// (un-adopt) and delete — close them here, all of them, because a branch
    /// may carry several. A worktree that vanishes takes its agents with it
    /// through the reaper instead.
    /// A merge that prunes its checkout takes the run's agents with it: the
    /// directory they live in is about to go, so their sessions end here,
    /// recorded on the thread, rather than lingering live until the reaper
    /// notices the root is gone.
    #[track_caller]
    pub(in crate::app) fn retire_agents_of_pruned_worktree(&mut self, root: &std::path::Path) {
        let root = Self::canonical_root(root);
        let ended: Vec<SessionInstance> = self
            .session_registry
            .tab_keys()
            .into_iter()
            .filter(|key| key.is_agent() && key.root == root)
            .filter_map(|key| self.session_registry.session_instance(&key))
            .collect();
        let _retiring = self.retire_agent_tabs(&root);
        for instance in ended {
            self.record_agent_session_end(&instance.entity_id, &instance.agent_id, &instance);
        }
    }

    #[track_caller]
    pub(in crate::app) fn retire_agent_tabs(&mut self, root: &std::path::Path) -> Vec<Retirement> {
        let caller = std::panic::Location::caller();
        let root = Self::canonical_root(root);
        let keys: Vec<TabKey> = self
            .session_registry
            .tab_keys()
            .into_iter()
            .filter(|key| key.is_agent() && key.root == root)
            .collect();
        keys.iter()
            .filter_map(|key| self.retire_tab_at(key, "closed", caller))
            .collect()
    }

    /// Stop every live agent and discard every not-yet-delivered turn scoped
    /// to one workspace root. Other workspaces may use the same owner shape,
    /// so the canonical tab root is the boundary rather than an id prefix.
    #[track_caller]
    pub(in crate::app) fn retire_workspace_agents(
        &mut self,
        root: &std::path::Path,
    ) -> Vec<Retirement> {
        let root = Self::canonical_root(root);
        let ended: Vec<SessionInstance> = self
            .session_registry
            .tab_keys()
            .into_iter()
            .filter(|key| key.is_agent() && key.root == root)
            .filter_map(|key| self.session_registry.session_instance(&key))
            .collect();
        self.delivery_queue
            .retain_queued(|turn| turn.tab_key().root != root);
        let retirements = self.retire_agent_tabs(&root);
        for instance in ended {
            self.record_agent_session_end(&instance.entity_id, &instance.agent_id, &instance);
        }
        retirements
    }

    /// Remove one tab, tell its clients `reason`, and retire its process.
    ///
    /// The kill and the reap leave for a thread of their own
    /// ([`Retirement`]); the close push stays here, under the app mutex,
    /// because it is bounded — the screen's own lock and one channel send per
    /// client, exactly what it has always been.
    #[track_caller]
    pub(in crate::app) fn retire_tab(&mut self, key: &TabKey, reason: &str) -> Option<Retirement> {
        self.retire_tab_at(key, reason, std::panic::Location::caller())
    }

    pub(in crate::app) fn retire_tab_at(
        &mut self,
        key: &TabKey,
        reason: &str,
        caller: &std::panic::Location<'_>,
    ) -> Option<Retirement> {
        let identity = self.session_registry.agent_snapshot(key).and_then(|tab| {
            tab.role
                .agent()
                .map(|(owner, agent)| (owner.to_string(), agent.to_string()))
        });
        let provider_thread_id = identity
            .as_ref()
            .and_then(|(owner, agent)| self.recorded_resume_id(owner, agent));
        let shell = self.session_registry.tab_is_shell(key);
        let retired = self
            .session_registry
            .retire_tab(
                key,
                reason,
                LifecycleDiagnostic {
                    event: "shutdown_requested",
                    origin: "tab_retirement",
                    reason: Some(reason),
                    operation_id: None,
                    provider_thread_id: provider_thread_id.as_deref(),
                    caller: Some(caller),
                },
            )
            .map(|retired| retired.retirement);
        // Only a shell: an agent tab is not one of the rows `term.list`
        // carries, so its coming and going moves no tab list.
        if retired.is_some() && shell {
            self.note_terminals_at(&key.root);
        }
        retired
    }

    /// Remove one tab and retire its process, keeping its screen for the
    /// session that replaces it.
    ///
    /// The clients are told nothing and stay attached, which is what keeps a
    /// browser's terminal where the human left it across an agent restart.
    /// [`ensure_agent_tab`]'s dead-tab replacement, and nothing else.
    #[track_caller]
    pub(in crate::app) fn retire_tab_keeping_screen(
        &mut self,
        key: &TabKey,
    ) -> Option<(Retirement, Option<ScreenHandle>)> {
        let identity = self.session_registry.agent_snapshot(key).and_then(|tab| {
            tab.role
                .agent()
                .map(|(owner, agent)| (owner.to_string(), agent.to_string()))
        });
        let provider_thread_id = identity
            .as_ref()
            .and_then(|(owner, agent)| self.recorded_resume_id(owner, agent));
        self.session_registry
            .retain_screen_for_replacement(
                key,
                LifecycleDiagnostic {
                    event: "shutdown_requested",
                    origin: "dead_tab_replacement",
                    reason: Some("replaced"),
                    operation_id: None,
                    provider_thread_id: provider_thread_id.as_deref(),
                    caller: Some(std::panic::Location::caller()),
                },
            )
            .map(|retired| (retired.retirement, retired.screen))
    }

    /// `agent.add` — give a branch another agent, with its own conversation.
    ///
    /// Branches only: a task carries exactly one agent session, because
    /// implementing a task is a handoff to a new agent on a branch rather
    /// than a second agent on the task itself. The agent is a record and a
    /// conversation; no process is spawned until something is said to it.
    ///
    /// The FIRST agent of a branch that had none also seeds the entity's legacy
    /// default. That field is only a creation template after migration: every
    /// existing agent keeps and edits its own settings, including agents on the
    /// same provider.
    /// What this device says a role runs on, when the caller asked for one.
    ///
    /// `None` when the caller named no role, and when the user has declared no
    /// model for the role they named — in both cases whatever the caller said
    /// itself, or the device default, stands. A role or capability word this
    /// bridge does not know is refused by name rather than ignored: a caller
    /// that asked for `"reviewing"` and silently got the default would never
    /// find out it had asked for nothing.
    ///
    /// Effort is deliberately absent. Which model fills a role is the user's
    /// call; how hard it thinks about one piece of work is the creating
    /// agent's, and it passes `effort` itself when it has a view.
    pub(in crate::app) fn role_choice_for(
        &self,
        params: &Value,
    ) -> Result<Option<(ModelChoice, crate::models::AgentCapability)>, String> {
        let word = |key: &str| {
            params
                .get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|word| !word.is_empty())
        };
        let Some(named) = word("role") else {
            return Ok(None);
        };
        let role = crate::models::AgentRole::from_wire(named).ok_or_else(|| {
            format!(
                "There is no {named:?} role. The roles are {}.",
                listed(crate::models::AgentRole::ALL.map(|role| role.wire_id()))
            )
        })?;
        let capability = match word("capability") {
            Some(named) => Some(crate::models::AgentCapability::from_wire(named).ok_or_else(
                || {
                    format!(
                        "There is no {named:?} capability. They are {}.",
                        listed(
                            crate::models::AgentCapability::ALL
                                .map(|capability| capability.wire_id())
                        )
                    )
                },
            )?),
            None => None,
        };
        let Some(entry) = self.role_model_here(role, capability) else {
            // Asked for something this device has nobody for. Saying so beats
            // quietly starting a model the user did not choose for the job.
            if let Some(wanted) = capability {
                return Err(format!(
                    "No model on this device is a {} {}. {}",
                    wanted.wire_id(),
                    role.wire_id(),
                    self.roles_on_offer()
                ));
            }
            return Ok(None);
        };
        Ok(Some((
            ModelChoice {
                provider: entry.provider.unwrap_or(self.default_harness),
                model: Some(entry.model.clone()),
                // The user chose the model; the creating agent chooses how
                // hard it thinks.
                effort: None,
            },
            entry.capability,
        )))
    }

    /// What this device HAS said, for a refusal that has to leave the caller
    /// somewhere to go.
    fn roles_on_offer(&self) -> String {
        let offered = self.role_models.roles_offered();
        if offered.is_empty() {
            return "No models have been given roles on this device yet; choose a model yourself, or leave it to the default.".to_string();
        }
        format!(
            "Roles with a model: {}.",
            offered
                .iter()
                .map(|role| role.wire_id())
                .collect::<Vec<_>>()
                .join(", ")
        )
    }

    pub(crate) fn agent_add(&mut self, params: &Value) -> Result<Value, String> {
        let entity_id = require_str(params, "entity_id")?;
        // Named at creation when the caller knows what it is making, which is
        // the usual case: whoever cuts an agent for a piece of work can say
        // what that work is better than the agent can before it has read
        // anything. Checked before anything is made, so a name that cannot be
        // had refuses rather than leaving an agent wearing an ordinal nobody
        // asked for.
        let name = match optional_nonempty_string(params, "name")? {
            Some(word) => Some(crate::agent::agent_name_from(word)?),
            None => None,
        };
        let creation_id = optional_nonempty_string(params, "creation_id")?.map(str::to_string);
        if creation_id.as_ref().is_some_and(|id| id.len() > 128) {
            return Err("agent.add: creation_id is too long".to_string());
        }
        if self.plans.contains_key(&entity_id) {
            return Err(crate::app::tasks::TASKS_RETIRED_ERROR.to_string());
        }
        if !self.runs.contains_key(&entity_id) {
            return Err(format!("agent.add: unknown entity {entity_id}"));
        }
        // The provider is parsed before anything is touched, so an unrunnable
        // one refuses instead of leaving an agent nothing can start.
        let existing_creation = creation_id.as_deref().and_then(|creation_id| {
            self.entity_agents(&entity_id)
                .ok()?
                .iter()
                .find(|agent| agent.creation_id.as_deref() == Some(creation_id))
                .cloned()
        });
        let retried_choice = existing_creation
            .as_ref()
            .and_then(|agent| agent.creation_choice.clone());
        // What the device says this ROLE runs on, when the caller asked for
        // one. It is the floor an explicit provider/model/effort is laid over,
        // so naming an effort does not discard the model the user chose.
        let (asked_for, direction) = match self.role_choice_for(params)? {
            Some((choice, capability)) => (Some(choice), Some(capability)),
            None => (None, None),
        };
        let choice = if has_agent_choice(params) {
            let beneath = match asked_for {
                Some(resolved) => resolved,
                None => ModelChoice {
                    provider: self.default_harness,
                    model: None,
                    effort: None,
                },
            };
            model_choice_over(params, beneath)?
        } else {
            match asked_for {
                Some(resolved) => resolved,
                None => retried_choice.unwrap_or(self.mint_model_choice(&entity_id)?),
            }
        };
        if let Some(existing) = existing_creation {
            if existing.creation_choice.as_ref() != Some(&choice) {
                return Err(format!(
                    "agent.add: creation_id {} was already used with different agent settings",
                    creation_id
                        .as_deref()
                        .expect("an existing creation has an id")
                ));
            }
            let root = self.entity_agent_root(&entity_id).ok();
            return Ok(json!({
                "entity_id": entity_id,
                "created": false,
                "agent": self.agent_digest(
                    &entity_id,
                    &existing,
                    root.as_deref(),
                    DigestScope::List,
                ),
            }));
        }
        // An agent nothing can start is refused before it is made. Only on a
        // fresh answer: this verb cannot wait on a CLI, and a stale one is
        // left to the spawn, which asks again before it refuses.
        if let Some(why) = held_refusal(&self.cli_readings, &choice) {
            return Err(why);
        }
        if let Some(name) = &name {
            let taken = self.entity_agents(&entity_id)?.iter().any(|agent| {
                agent
                    .name
                    .as_deref()
                    .is_some_and(|theirs| crate::agent::same_agent_name(theirs, name))
            });
            if taken {
                return Err(format!(
                    "Another agent on this conversation is already called \"{name}\". Pick a different name."
                ));
            }
        }
        let before_agents = self.runs[&entity_id].agents.clone();
        let before_entity_choice = self.runs[&entity_id].model_choice.clone();
        let owner = self.agent_owner(&entity_id);
        let mut active = self.take_run(&entity_id)?;
        if active.agents.is_empty() {
            active.model_choice = choice.clone();
        }
        let (added, created) = match creation_id.as_deref() {
            Some(creation_id) => active
                .agents
                .add_idempotent(owner, choice, &now_rfc3339(), creation_id)
                .expect("the creation id and any prior use were validated before taking the run"),
            None => (
                active.agents.add(owner, choice, &now_rfc3339()).clone(),
                true,
            ),
        };
        if let Some(agent) = active.agents.iter_mut().find(|agent| agent.id == added.id) {
            if let Some(name) = &name {
                agent.name = Some(name.clone());
                // Given a name, so never asked for one.
                agent.name_asked = true;
            }
            // Whether the user sees this conversation in their inbox. A
            // creation the UI made is the user's own and is watched; one an
            // agent made for itself is not, until it says `notify_user` —
            // otherwise an agent spawning three puts three rows in front of
            // somebody who asked for one thing.
            agent.watched = watching_asked(params);
            // Which agent's Build MCP call this is (#216). Only the bridge's
            // own callers set it: `agent.add` over the wire declares no such
            // param, so a client cannot claim a creator.
            agent.created_by = params
                .get("created_by")
                .and_then(Value::as_str)
                .map(str::to_string);
        }
        let added = active
            .agents
            .iter()
            .find(|agent| agent.id == added.id)
            .cloned()
            .unwrap_or(added);
        let persisted = self.finish_run_mutation(entity_id.clone(), active);
        if let Err(error) = persisted {
            let restored = self
                .runs
                .get_mut(&entity_id)
                .expect("the failed finish put the run back");
            restored.agents = before_agents;
            restored.model_choice = before_entity_choice;
            return Err(error);
        }
        self.touch_attention(&entity_id);
        let root = self.entity_agent_root(&entity_id).ok();
        let mut answered = json!({
            "entity_id": entity_id,
            "created": created,
            "agent": self.agent_digest(&entity_id, &added, root.as_deref(), DigestScope::List),
        });
        // How much direction the model this role resolved to wants. The point
        // of asking for a role: the caller is about to write this agent's
        // brief, and a one-line brief to a step-by-step model is the mistake
        // the answer exists to stop.
        if let Some(capability) = direction {
            answered["capability"] = json!(capability);
            answered["direction"] = json!(capability.describes());
        }
        Ok(answered)
    }

    /// `agent.choose` — set the model and reasoning effort one exact agent runs
    /// on. The composer's model menu, and the only verb that persists an
    /// agent-owned choice without spawning anything.
    ///
    /// The provider is not a question here: an agent is locked to the harness
    /// it was created on, and a caller that names one is refused by that
    /// harness's name. A live session is untouched —
    /// the choice is what the NEXT start spends, which is exactly what the
    /// menu offers.
    pub(crate) fn agent_choose(&mut self, params: &Value) -> Result<Value, String> {
        let entity_id = require_str(params, "entity_id")?;
        if self.plans.contains_key(&entity_id) {
            return Err(crate::app::tasks::TASKS_RETIRED_ERROR.to_string());
        }
        let requested_agent = named_agent_id(params)?;
        let agent = self
            .entity_agents(&entity_id)?
            .resolve(requested_agent.as_deref())?;
        let agent_id = agent.id.clone();
        let locked = agent.choice.provider;
        if let Some(expected) = optional_nonempty_string(params, "conversation_id")? {
            if expected != agent.conversation_id() {
                return Err(format!(
                    "agent.choose: stale conversation_id {expected}; agent {agent_id} is bound to {}",
                    agent.conversation_id()
                ));
            }
        }
        if let Some(named) = params.get("provider").and_then(Value::as_str) {
            if !named.is_empty() {
                return Err(format!(
                    "agent.choose: the agent is locked to {} — model and effort only",
                    locked.label()
                ));
            }
        }
        let choice = model_choice_from(params, locked)?;
        let expected_revision = params
            .get("expected_choice_revision")
            .map(|value| {
                value.as_u64().ok_or_else(|| {
                    "agent.choose: expected_choice_revision must be an unsigned integer".to_string()
                })
            })
            .transpose()?;
        if let Some(expected) = expected_revision {
            if expected != agent.choice_revision {
                return Err(format!(
                    "agent.choose: stale choice revision {expected}; current revision is {}",
                    agent.choice_revision
                ));
            }
        }
        let choice_revision = self.set_agent_model_choice(&entity_id, &agent_id, choice.clone())?;
        Ok(json!({
            "entity_id": entity_id,
            "agent_id": agent_id,
            "provider": choice.provider,
            "model": choice.model,
            "effort": choice.effort,
            "choice_revision": choice_revision,
        }))
    }

    pub(in crate::app) fn set_agent_model_choice(
        &mut self,
        entity_id: &str,
        agent_id: &str,
        choice: ModelChoice,
    ) -> Result<u64, String> {
        let previous = self
            .entity_agents(entity_id)?
            .resolve(Some(agent_id))?
            .clone();
        let revision = if self.plans.contains_key(entity_id) {
            let mut active = self.take_plan(entity_id)?;
            let revision = active
                .agents
                .resolve_mut(Some(agent_id))
                .expect("the agent was validated before its task was taken")
                .choose(choice);
            if let Err(error) = self.finish_plan_mutation(entity_id.to_string(), active) {
                *self
                    .plans
                    .get_mut(entity_id)
                    .expect("the failed finish put the task back")
                    .agents
                    .resolve_mut(Some(agent_id))
                    .expect("the previous agent still belongs to the task") = previous;
                return Err(error);
            }
            revision
        } else if self.runs.contains_key(entity_id) {
            let mut active = self.take_run(entity_id)?;
            let revision = active
                .agents
                .resolve_mut(Some(agent_id))
                .expect("the agent was validated before its run was taken")
                .choose(choice);
            if let Err(error) = self.finish_run_mutation(entity_id.to_string(), active) {
                *self
                    .runs
                    .get_mut(entity_id)
                    .expect("the failed finish put the run back")
                    .agents
                    .resolve_mut(Some(agent_id))
                    .expect("the previous agent still belongs to the run") = previous;
                return Err(error);
            }
            revision
        } else {
            return Err("unknown id".to_string());
        };
        self.note_entity_changed(entity_id);
        Ok(revision)
    }

    /// `agent.remove` — take an agent back off a branch's rail.
    ///
    /// The mirror of [`agent_add`](Self::agent_add), and it validates the same
    /// way: branches only, because a task's one agent IS the task's
    /// conversation — there is nothing to remove there, only a task to
    /// abandon.
    ///
    /// Any of a branch's agents may go, the primary and the last one included.
    /// A branch with none is a working branch: its chat tab shows the
    /// new-agent view, and the next thing the system has to say to it mints an
    /// agent through [`ensure_primary_agent`](Self::ensure_primary_agent).
    ///
    /// A removed agent's harness must not outlive it. An agent with no roster
    /// entry keeps working in the checkout and reports `done` for an identity
    /// nothing can route to — the same hazard
    /// [`retire_agent_tabs`](Self::retire_agent_tabs) exists for — so its session is
    /// killed and reaped and everything that could reach it goes too.
    pub(crate) fn agent_remove(&mut self, params: &Value) -> Result<Value, String> {
        let entity_id = require_str(params, "entity_id")?;
        let agent_id = require_str(params, "agent_id")?;
        if self.plans.contains_key(&entity_id) {
            return Err(crate::app::tasks::TASKS_RETIRED_ERROR.to_string());
        }
        if !self.runs.contains_key(&entity_id) {
            return Err(format!("agent.remove: unknown entity {entity_id}"));
        }
        let root = self.entity_agent_root(&entity_id)?;
        // A harness being spawned right now cannot be killed: the tab it will
        // land in does not exist yet, so the reservation is the only handle on
        // it, and the human can ask again a moment later.
        if self
            .session_registry
            .claim_is_held(&TabKey::agent(&root, &agent_id))
        {
            return Err(format!(
                "agent.remove: {agent_id} is starting a session right now — remove it once the \
                 session is running"
            ));
        }
        self.preserve_entity_task_identities(&entity_id)?;
        let task_refresh = self.tasks_with_agent_identity(&entity_id, &agent_id)?;
        let removed_agent_revived_clear = self
            .entity_agents(&entity_id)
            .ok()
            .and_then(|roster| {
                let position = roster.iter().position(|agent| agent.id == agent_id)?;
                let agent = roster.by_id(&agent_id)?;
                let thread = if roster.is_primary(&agent.id) {
                    self.entity_conversation(&entity_id)
                        .unwrap_or(&agent.thread)
                } else {
                    &agent.thread
                };
                let attention = self.board.attention().attention(&entity_id)?;
                let sequence = thread.last_own_message_sequence();
                let crossed = match attention.dismissed_line_for(&agent.id, position == 0) {
                    Some(line) => sequence > line,
                    None => sequence > 0 && attention.has_message_dismissal(),
                };
                Some(crossed)
            })
            .unwrap_or(false);
        let mut active = self.take_run(&entity_id)?;
        let removed = match active.agents.remove(&agent_id) {
            Ok(removed) => removed,
            Err(refused) => {
                // Nothing was touched, so the run goes back exactly as it came.
                self.runs.insert(entity_id, active);
                return Err(format!("agent.remove: {refused}"));
            }
        };
        let persisted = self.finish_run_mutation(entity_id.clone(), active);
        // finish_run_mutation installs the changed live roster even if its
        // persistence fails. Invalidate wherever that availability is cached;
        // tasks.get/list retain the historical facts and resolve availability.
        if let Some((project_id, task_ids)) = task_refresh {
            self.changes.note_tasks(&project_id, &task_ids);
        }
        self.retire_agent(&root, &removed.id);
        // Nothing prunes cursors by agent, so one left behind here would
        // outlive the daemon it was written in.
        self.board.attention_mut().remove_agent_cursor(
            &entity_id,
            &removed.id,
            removed_agent_revived_clear,
        );
        self.persist_attention();
        persisted?;
        self.touch_attention(&entity_id);
        Ok(json!({
            "entity_id": entity_id,
            "agent_id": removed.id,
            "agents": self.agent_digests(&entity_id, DigestScope::List),
        }))
    }

    /// Kill, reap and forget ONE agent's session, plus everything else that
    /// could still reach it: the capability its harness authenticates control
    /// frames with, the screen a client is waiting on a first spawn for, and
    /// any turn still queued to be said to it.
    ///
    /// The per-agent twin of [`retire_agent_tabs`](Self::retire_agent_tabs), which
    /// takes every agent in a worktree because its owner is going away.
    #[track_caller]
    pub(in crate::app) fn retire_agent(&mut self, root: &std::path::Path, agent_id: &str) {
        let key = TabKey::agent(&Self::canonical_root(root), agent_id);
        self.retire_tab(&key, "closed");
        self.session_registry.remove_waiting_screen(&key, "closed");
        self.session_registry.revoke_mcp_token(agent_id);
        self.delivery_queue
            .retain_queued(|turn| turn.agent_id != agent_id);
    }

    /// End a task's agent session, because the gate that just closed ended
    /// it. A task agent works in the PRIMARY checkout, which never goes
    /// away, so nothing else would ever stop it: it would keep working there
    /// and report `done` for a task no longer taking reports. Only this
    /// task's own agent goes — the checkout's other agents belong to the main
    /// branch and are none of this verb's business.
    #[track_caller]
    pub(in crate::app) fn retire_task_session(
        &mut self,
        session: Option<(std::path::PathBuf, String)>,
    ) {
        if let Some((checkout, agent_id)) = session {
            self.retire_agent(&checkout, &agent_id);
        }
    }

    /// The entity's agents, in rail order: what `list_workspace_agents` and a
    /// workspace's detail read.
    pub(crate) fn agent_list(&mut self, params: &Value) -> Result<Value, String> {
        let entity_id = require_str(params, "entity_id")?;
        Ok(json!({
            "entity_id": entity_id,
            "agents": self.agent_digests(&entity_id, DigestScope::List),
        }))
    }

    /// What the rail's bubble strip renders for one entity: one digest per
    /// agent, in rail order.
    pub(in crate::app) fn agent_digests(&self, entity_id: &str, scope: DigestScope) -> Vec<Value> {
        let Ok(roster) = self.entity_agents(entity_id) else {
            return Vec::new();
        };
        let root = self.entity_agent_root(entity_id).ok();
        roster
            .iter()
            .map(|agent| self.agent_digest(entity_id, agent, root.as_deref(), scope))
            .collect()
    }

    /// Conversation identities for rail navigation; inbox sessions live on
    /// workspace and project rows, pooling messages from all their agents.
    pub(in crate::app) fn conversation_activity_rows(&self, entity_id: &str) -> Vec<Value> {
        let Ok(roster) = self.entity_agents(entity_id) else {
            return Vec::new();
        };
        let mut seen = std::collections::HashSet::new();
        roster
            .iter()
            .filter_map(|agent| {
                let conversation_id = agent.conversation_id();
                if !seen.insert(conversation_id.to_string()) {
                    return None;
                }
                Some(json!({
                    "conversation_id": conversation_id,
                }))
            })
            .collect()
    }

    /// One bubble: who the agent is, what it runs on, whether it is live, and
    /// how much of its conversation is waiting for the human.
    ///
    /// Unread is counted from the agent's canonical conversation binding.
    pub(in crate::app) fn agent_digest(
        &self,
        entity_id: &str,
        agent: &crate::agent::Agent,
        root: Option<&std::path::Path>,
        scope: DigestScope,
    ) -> Value {
        let thread = self
            .agent_conversation(entity_id, Some(&agent.id))
            .unwrap_or(&agent.thread);
        let unread = self.agent_unread(entity_id, agent, thread);
        let tab = root.map(|root| TabKey::agent(root, &agent.id));
        let tab = tab.as_ref().and_then(|key| {
            self.session_registry
                .agent_digest_facts(key, matches!(scope, DigestScope::Detail))
        });
        let live = tab.as_ref().is_some_and(|tab| tab.live);
        let next_start = agent.choice.clone();
        let mut digest = json!({
            "id": agent.id,
            "watched": agent.watched,
            "conversation_id": agent.conversation_id(),
            "ordinal": agent.ordinal,
            "provider": agent.choice.provider,
            "model": next_start.model.clone().unwrap_or_default(),
            "effort": next_start.effort.clone().unwrap_or_default(),
            "active_model": agent
                .active_model
                .clone()
                .or(next_start.model)
                .unwrap_or_default(),
            "active_effort": agent
                .active_effort
                .clone()
                .unwrap_or_default(),
            "state": if live {
                crate::agent::AgentLifecycle::Live.as_str()
            } else {
                agent.state.as_str()
            },
            "unread_count": unread.count,
            "unread_reason": unread.reason,
            // Where the reader got to, so the panel can rule its unread divider
            // and open on the first message they have not seen.
            "read_through_sequence": self.read_cursor(entity_id, &agent.id),
            "working": tab.as_ref().is_some_and(|tab| tab.working),
            "working_time": working_time_json(agent.working_since.as_deref()),
            "choice_revision": agent.choice_revision,
            // Whether the rail offers this agent a basement. The live session
            // answers for an agent that is running, since it is the only thing
            // that can; before there is one the PROVIDER answers, because it
            // knows whether its spawn will open a terminal. Same authority either
            // side of the spawn, so the rail never offers a TUI button that the
            // spawn then refuses.
            "has_terminal": match &tab {
                Some(tab) => tab.has_terminal,
                None => harness_for(agent.choice.provider).has_terminal(),
            },
            // Whether the composer offers "Interrupt & send". Unlike
            // `has_terminal` the PROVIDER cannot answer this one: the capability
            // is announced by the child in its own `init` line rather than
            // decided by the argv, so the same provider answers differently on
            // two versions of the same CLI. No session, no turn to stop.
            "can_interrupt": tab.as_ref().is_some_and(|tab| tab.can_interrupt),
            // Surfaces are process-local observations. The client scopes its
            // cache to this instance identity so a restarted process cannot
            // inherit the previous process's goal or checklist snapshot.
            "surface_session_generation": tab
                .as_ref()
                .and_then(|tab| tab.surface_session_generation.clone()),
            // Why the last turn queued for this agent never reached a harness.
            // The client's "starting" state is laid on before there is any
            // session to report, and this is what takes it off when none ever
            // opened — the only word a start that failed ever gets to say.
            "start_error": agent.start_error,
            "created_at": agent.created_at,
            // What the agent named its conversation, for the header to wear in
            // place of the harness name. Null until it has, which the client
            // shows as "Starting".
            "topic": agent.topic,
            // What to CALL it, everywhere it used to be "Agent 1". Null until
            // somebody names it, and a client that has never heard of the
            // field — or is looking at an agent that has no name yet — falls
            // back to the ordinal, which has not moved.
            "name": agent.name,
            // The agent whose Build MCP call made this one (#216), for the
            // creator's activity panel; null for one the user made.
            "created_by": agent.created_by,
            // What the last turn left in context and what the session's cache
            // reads have come to, as the harness reported them; null until it
            // has. The threshold is the conversation's own when it set one,
            // and `compact_at_tokens` is the one in effect (0: never).
            "last_context_tokens": agent.last_context_tokens,
            // When that reading was recorded; null exactly when it is.
            "last_context_at": agent.last_context_at,
            "session_cache_read_tokens": agent.session_cache_read_tokens,
            "max_context_tokens": agent.max_context_tokens,
            "compact_at_tokens": agent.compact_at_tokens(self.compact_above_tokens),
            // What to CALL this conversation in a list: the agent's own topic,
            // or — until it sets one — the first line the human opened with.
            // Null for a conversation with neither, which is one nothing has
            // been said in yet.
            "title": agent
                .topic
                .clone()
                .or_else(|| thread.first_user_line(TITLE_MAX_CHARS)),
        });
        if let Some(surfaces) = tab.and_then(|tab| tab.surfaces) {
            digest["surfaces"] = surfaces;
        }
        digest
    }

    /// Write down what a session said about itself.
    ///
    /// Compared before it is written, so a session that names its conversation
    /// once costs one write however long it lives. A name that has not arrived
    /// leaves the record alone: what it carries is the last session's, which is
    /// exactly what a resume should use if this one dies before naming its own.
    ///
    /// Both carriers' capture points come through here, so a name a child
    /// announced and a name a locator found are the same record written by the
    /// same hand.
    pub(in crate::app) fn note_self_report(
        &mut self,
        owner: &str,
        agent_id: &str,
        instance: &SessionInstance,
        report: SelfReport,
    ) {
        if let Some(named) = report.named {
            if let Err(error) =
                self.edit_agent_conversation(owner, agent_id, |thread, _artifact| {
                    if thread.name_session_instance(instance, &named) {
                        Ok(())
                    } else {
                        Err(
                            "session instance no longer matches its conversation lineage"
                                .to_string(),
                        )
                    }
                })
            {
                eprintln!("note_self_report {owner}: {error}");
                return;
            }
            if self.recorded_resume_id(owner, agent_id).as_deref() != Some(named.as_str()) {
                self.record_agent_resume_id(owner, agent_id, Some(named));
            }
        }
        if report.model.is_some() {
            self.record_agent_runtime_choice(owner, agent_id, report.model, report.effort);
        }
    }
}
