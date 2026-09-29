//! Agents: the thing a conversation belongs to.
//!
//! An agent is a first-class, durable identity — not a harness process. The
//! process dies on every restart and context clear; the agent, its provider
//! choice, its display ordinal and above all its conversation outlive it.
//!
//! One stable conversation binding per agent (spec: UX Redesign Decisions,
//! "Agents and conversations"). A branch can carry any number of agents working the same
//! checkout — including none, which is what a branch starts with — while a
//! task carries exactly one. So an entity holds an [`AgentRoster`] rather
//! than a thread, and the agent at index 0 is the PRIMARY: the one every
//! entity-level event speaks to, and the one a verb that names no agent means.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::models::ModelChoice;
use crate::thread::Thread;

/// What every agent id starts with, so an id read off the wire says what kind
/// of thing it names without a lookup.
pub const AGENT_ID_PREFIX: &str = "agent-";

/// Version of the persisted per-agent settings contract written today.
///
/// Zero is deliberately reserved for records written before settings became
/// agent-owned. [`AgentRoster::restore`] materializes their old effective
/// choice once, then advances the marker so later entity edits cannot move it.
pub const CURRENT_SETTINGS_VERSION: u8 = 1;

/// What every project agent's id starts with. Load-bearing exactly as the
/// router's prefix is: the MCP control plane reads the surface off the id
/// alone, so a project agent is handed the project tools without anything
/// having to remember to say which surface it is on.
pub const PROJECT_AGENT_ID_PREFIX: &str = "project-";

/// Whether an id names the agent of a project's conversation owner rather than
/// an agent that works a checkout.
pub fn is_project_agent(agent_id: &str) -> bool {
    agent_id.starts_with(PROJECT_AGENT_ID_PREFIX)
}

/// Which kind of agent an owner's roster mints.
///
/// The kind is a property of the OWNER, not of the verb that happened to reach
/// it: a project's conversation owner holds no checkout and so its agents get
/// the project surface, whoever asked for them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AgentKind {
    /// An agent of a branch, a task or a workspace. It works a checkout.
    #[default]
    Coding,
    /// An agent of a project's conversation owner. It holds no checkout and
    /// reaches Build through the project surface.
    Project,
}

impl AgentKind {
    /// A fresh id of this kind. The prefix IS the kind, read back by
    /// [`crate::mcp::McpSurface::for_owner`].
    pub fn new_id(self) -> String {
        match self {
            AgentKind::Coding => new_agent_id(),
            AgentKind::Project => new_project_agent_id(),
        }
    }
}

/// Who a roster is minting for: the owner's id, and the kind of agent that
/// owner has.
///
/// A bare `&str` owner converts to the coding kind, which is what every branch,
/// task and workspace is; the project case has to be named, and is named where
/// the owner is recognized rather than where the agent is asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentOwner<'a> {
    pub id: &'a str,
    pub kind: AgentKind,
}

impl<'a> AgentOwner<'a> {
    /// The conversation owner of a project.
    pub fn project(id: &'a str) -> AgentOwner<'a> {
        AgentOwner {
            id,
            kind: AgentKind::Project,
        }
    }
}

impl<'a> From<&'a str> for AgentOwner<'a> {
    fn from(id: &'a str) -> AgentOwner<'a> {
        AgentOwner {
            id,
            kind: AgentKind::Coding,
        }
    }
}

impl<'a> From<&'a String> for AgentOwner<'a> {
    fn from(id: &'a String) -> AgentOwner<'a> {
        AgentOwner::from(id.as_str())
    }
}

/// Crockford base32 — ULID's alphabet: no I, L, O or U, so an id read aloud or
/// typed by hand cannot become a different id.
const CROCKFORD: [u8; 32] = *b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// The character length of a ULID body (128 bits in base32).
const ULID_LEN: usize = 26;

/// Where an agent is in its life. Lifecycle, never process state: `Live` means
/// a harness is running for it right now, `Idle` that it exists and is not
/// running, `Ended` that it has been retired and will not run again.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentLifecycle {
    Live,
    #[default]
    Idle,
    Ended,
}

impl AgentLifecycle {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentLifecycle::Live => "live",
            AgentLifecycle::Idle => "idle",
            AgentLifecycle::Ended => "ended",
        }
    }
}

/// One agent of one entity: its identity, what it runs on, where it sits in the
/// rail, and the canonical conversation it is bound to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Agent {
    pub id: String,
    /// The plan/run this agent belongs to. An agent is never free-floating: it
    /// is reachable only through its branch or task.
    pub owner_id: String,
    #[serde(default)]
    pub choice: ModelChoice,
    /// Monotonic version of this agent's model/effort selection. Clients use it
    /// to discard a digest that raced with the answer to `agent.choose`.
    #[serde(default)]
    pub choice_revision: u64,
    /// Migration marker for the point at which `choice` became agent-owned.
    #[serde(default)]
    pub settings_version: u8,
    /// Client operation id that created this rail entry, when creation was
    /// requested through the idempotent `agent.add` API.
    #[serde(default)]
    pub creation_id: Option<String>,
    /// Effective settings of that creation request. Kept separately because
    /// the agent's current choice may legitimately change before a retry lands.
    #[serde(default)]
    pub creation_choice: Option<ModelChoice>,
    /// 1-based display position in the agent rail. Stable for the agent's life:
    /// removing an agent must not renumber the ones beside it.
    pub ordinal: u32,
    pub created_at: String,
    #[serde(default)]
    pub state: AgentLifecycle,
    /// The locally stored thread when this agent owns its canonical history.
    /// An alias keeps this empty and resolves through `conversation_id`.
    #[serde(default)]
    pub thread: Thread,
    /// Agent id whose thread stores this agent's conversation.
    ///
    /// Usually this agent's own id. An implementation agent binds to the Task
    /// agent whose conversation it continues. `None` exists only while reading
    /// a legacy record and is materialized before that record is written again.
    #[serde(default)]
    pub conversation_id: Option<String>,
    /// The name the agent's last session gave the conversation it was having,
    /// for a respawn to resume BY NAME rather than by guessing the newest
    /// transcript in the checkout.
    ///
    /// `None` for a carrier that names no conversation, for an agent that has
    /// never run, and for one whose session died before it could announce
    /// itself — all three of which fall back to the transcript probe, which is
    /// what shipped before this and still works. Never an error: this is a
    /// sharpening of a path that already answers.
    #[serde(default)]
    pub resume_session_id: Option<String>,
    #[serde(default)]
    pub active_model: Option<String>,
    /// The reasoning effort last reported by the running session.
    #[serde(default)]
    pub active_effort: Option<String>,
    /// When the agent's current working interval began, if it is working now.
    #[serde(default)]
    pub working_since: Option<String>,
    /// Why the last turn queued for this agent never reached a harness.
    ///
    /// A start is answered before the harness exists, so the client lays a
    /// "starting" state over the row and waits for this agent's next word. A
    /// spawn that never came up says nothing about the session, and this is
    /// that word: the reason, until the next turn on its way replaces it.
    /// `None` for an agent whose last delivery landed, and for one that has
    /// never been asked to run.
    #[serde(default)]
    pub start_error: Option<String>,
    /// The agent's own word for what this conversation is about — a 2-4 word
    /// objective it sets over MCP (`set_topic`), which the conversation header
    /// wears in place of the harness name. `None` until it has: the header
    /// says "Starting" until then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    /// What to CALL this agent: one or two meaningful words, set when it was
    /// created or chosen by the agent itself over MCP (`set_name`).
    ///
    /// Not the topic. A topic is a subject line and moves with the work — an
    /// agent that finished one thing and started another sets a new one. A
    /// name is who the agent IS, it is unique among the agents of its
    /// conversation, and everything that used to say "Agent 1" says this
    /// instead. `None` until somebody sets one, and the ordinal is the
    /// fallback for exactly as long as that lasts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The agent whose Build MCP call made this one (#216): an
    /// `add_workspace_agent`, or an `assign_task` to a new agent or a new
    /// workspace. `None` for an agent the user made, and for every agent made
    /// before the field existed. The client lists an agent's creations in its
    /// activity panel and counts it running while any of them runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,
    /// Whether this agent has already been asked to name itself.
    ///
    /// The ask rides the first turn of an unnamed agent, and
    /// once only: an agent that was asked and did not do it is one that
    /// decided, and asking again every time the user speaks would be nagging
    /// with the user's own words. Never set for an agent that has a name.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub name_asked: bool,
    /// Whether the USER is watching this conversation (spec: Tasks →
    /// Watching).
    ///
    /// Every conversation used to be in the inbox whether the user had
    /// anything to do with it or not, which is how an agent spawning three
    /// agents put three rows in front of somebody who asked for one thing.
    /// Now the user watches what they made, and what an agent asked them to
    /// see.
    ///
    /// Defaults TRUE, unlike a task's. Every conversation that exists today
    /// is in the inbox, and a field that defaulted false would empty it on the
    /// first read after an upgrade. What narrows it is the creating paths
    /// setting it false for an agent an agent made.
    #[serde(default = "watched_by_default")]
    pub watched: bool,
    /// Tokens in context on the last turn this agent's session reported, or
    /// `None` until it reports one — and again once a compaction has been
    /// asked for, so a reading from before it cannot ask for another.
    #[serde(default)]
    pub last_context_tokens: Option<u64>,
    /// When [`last_context_tokens`](Self::last_context_tokens) was recorded,
    /// RFC3339 — what a reading an agent's words carry says it was taken at.
    #[serde(default)]
    pub last_context_at: Option<String>,
    /// Cache-read tokens the agent's current session process has spent over
    /// all its turns, as the harness last reported them.
    #[serde(default)]
    pub session_cache_read_tokens: Option<u64>,
    /// This conversation's own compaction threshold: `None` follows the
    /// device's, `Some(0)` never compacts, `Some(n)` compacts at `n` tokens.
    #[serde(default)]
    pub max_context_tokens: Option<u64>,
}

/// Conversations are watched unless somebody says otherwise — see
/// [`Agent::watched`].
fn watched_by_default() -> bool {
    true
}

/// The context size a device compacts its agents at until the user says
/// otherwise — well inside every harness's window, and far enough past a fresh
/// session's system prompt that a short conversation never pays for one.
pub const DEFAULT_COMPACT_ABOVE_TOKENS: u64 = 200_000;

/// The longest a name may be, and the most words it may have. Both are about
/// the rail: a name is drawn in a bubble and read in a line beside other
/// names, and one that does not fit is one the reader never sees whole.
pub const MAX_AGENT_NAME_CHARS: usize = 24;
pub const MAX_AGENT_NAME_WORDS: usize = 3;
pub const MIN_AGENT_NAME_CHARS: usize = 2;

/// A name as it will be stored, or a plain sentence saying why not.
///
/// Trimmed, and inner runs of whitespace collapsed, so "Rail   scroll" and
/// "Rail scroll" are the one name and cannot both be taken. Counted in
/// characters rather than bytes, because the limit is about what fits in a
/// bubble and a two-character name is two characters in any script.
pub fn agent_name_from(word: &str) -> Result<String, String> {
    let name = word.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.chars().count() < MIN_AGENT_NAME_CHARS {
        return Err(format!(
            "An agent's name needs at least {MIN_AGENT_NAME_CHARS} characters."
        ));
    }
    if name.chars().count() > MAX_AGENT_NAME_CHARS {
        return Err(format!(
            "An agent's name can be at most {MAX_AGENT_NAME_CHARS} characters, and that one is {}.",
            name.chars().count()
        ));
    }
    let words = name.split(' ').count();
    if words > MAX_AGENT_NAME_WORDS {
        return Err(format!(
            "An agent's name is one or two words — at most {MAX_AGENT_NAME_WORDS} — and that one is {words}."
        ));
    }
    Ok(name)
}

/// Whether two names are the same name. Case and spacing are not what make a
/// name different: "Rail scroll" and "rail  scroll" name one agent, and a rail
/// showing both would be a rail nobody can use.
pub fn same_agent_name(one: &str, other: &str) -> bool {
    one.split_whitespace()
        .map(str::to_lowercase)
        .eq(other.split_whitespace().map(str::to_lowercase))
}

impl Agent {
    /// A brand new agent with an empty conversation keyed to its own id.
    pub fn new(
        id: impl Into<String>,
        owner_id: impl Into<String>,
        choice: ModelChoice,
        ordinal: u32,
        created_at: impl Into<String>,
    ) -> Agent {
        let id = id.into();
        Agent {
            thread: Thread::for_agent(&id),
            conversation_id: Some(id.clone()),
            id,
            owner_id: owner_id.into(),
            choice,
            choice_revision: 0,
            settings_version: CURRENT_SETTINGS_VERSION,
            creation_id: None,
            creation_choice: None,
            ordinal,
            created_at: created_at.into(),
            state: AgentLifecycle::Idle,
            resume_session_id: None,
            active_model: None,
            active_effort: None,
            working_since: None,
            start_error: None,
            topic: None,
            name: None,
            name_asked: false,
            created_by: None,
            watched: true,
            last_context_tokens: None,
            last_context_at: None,
            session_cache_read_tokens: None,
            max_context_tokens: None,
        }
    }

    /// The context size at which this agent's next warm turn is preceded by a
    /// compaction: its own threshold, else the device's. 0 is never.
    pub fn compact_at_tokens(&self, device_threshold: u64) -> u64 {
        self.max_context_tokens.unwrap_or(device_threshold)
    }

    /// Whether the last reported context has reached the threshold. Inclusive,
    /// and never for an agent whose context nobody has reported.
    pub fn compaction_due(&self, device_threshold: u64) -> bool {
        let threshold = self.compact_at_tokens(device_threshold);
        threshold > 0
            && self
                .last_context_tokens
                .is_some_and(|tokens| tokens >= threshold)
    }

    /// The context this agent last reported, as what it writes carries it:
    /// the tokens, when they were recorded, the window of the model it runs —
    /// the one running now, else the one chosen — and where its chat compacts
    /// under `device_threshold`, absent when it never does. `None` without a
    /// recorded reading, which includes one cleared by a compaction.
    pub fn context_reading(&self, device_threshold: u64) -> Option<crate::thread::ContextReading> {
        let tokens = self.last_context_tokens?;
        let at = self.last_context_at.clone()?;
        let window = self
            .active_model
            .as_deref()
            .or(self.choice.model.as_deref())
            .and_then(|model| crate::models::context_window_of(self.choice.provider, model));
        let compact_at =
            Some(self.compact_at_tokens(device_threshold)).filter(|threshold| *threshold > 0);
        Some(crate::thread::ContextReading {
            tokens,
            window,
            compact_at,
            at,
        })
    }

    /// The durable storage identity of this agent's conversation.
    pub fn conversation_id(&self) -> &str {
        self.conversation_id.as_deref().unwrap_or(&self.id)
    }

    /// Bind this agent to an existing canonical conversation.
    pub fn bind_conversation(&mut self, conversation_id: impl Into<String>) {
        self.conversation_id = Some(conversation_id.into());
    }

    /// Persist a new per-agent setting and advance its wire revision.
    pub fn choose(&mut self, choice: ModelChoice) -> u64 {
        self.choice = choice;
        self.settings_version = CURRENT_SETTINGS_VERSION;
        self.choice_revision = self.choice_revision.saturating_add(1);
        self.choice_revision
    }
}

/// An entity's agents, in rail order — possibly none of them.
///
/// One agent of a stored record as a name to show: who it was, never what it
/// said. See [`AgentRoster::restored_members`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RosterMember {
    pub id: String,
    pub name: Option<String>,
    pub ordinal: u32,
    pub provider: crate::models::AgentProvider,
}

/// A branch starts with no agents and may be emptied back to none; a task
/// always holds exactly one. The agent at index 0 is the PRIMARY: the one
/// entity-level events speak to, and the one a verb that names no agent means.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AgentRoster {
    agents: Vec<Agent>,
}

impl AgentRoster {
    /// A roster with nobody on it — what a branch is created with.
    pub fn empty() -> AgentRoster {
        AgentRoster { agents: Vec::new() }
    }

    /// A roster holding one freshly minted agent — what a task gets, and
    /// what a dispatch that is about to speak gets.
    pub fn with_first(owner_id: &str, choice: ModelChoice, now: &str) -> AgentRoster {
        AgentRoster {
            agents: vec![Agent::new(
                derived_agent_id(owner_id),
                owner_id,
                choice,
                1,
                now,
            )],
        }
    }

    /// Rebuild a roster from what was persisted.
    ///
    /// `legacy` is the entity-keyed conversation records written before agents
    /// existed carry. It becomes the first agent's conversation, re-keyed to
    /// that agent — and because the first agent's id is DERIVED from the owner,
    /// running this again on an already-migrated record produces the same
    /// roster rather than a second agent.
    ///
    /// A record with no agents and no legacy conversation is not a record from
    /// before agents: it is an entity whose agents were all removed, or one
    /// created without any. It restores as the empty roster it is — the
    /// migration only ever runs where there is a conversation to carry across.
    pub fn restore(
        owner_id: &str,
        agents: Vec<Agent>,
        legacy: Thread,
        choice: ModelChoice,
        created_at: &str,
    ) -> AgentRoster {
        if agents.is_empty() && !legacy.is_empty() {
            let id = derived_agent_id(owner_id);
            let mut thread = legacy;
            thread.rekey_to_agent(&id);
            let mut first = Agent::new(id, owner_id, choice, 1, created_at);
            first.thread = thread;
            return AgentRoster {
                agents: vec![first],
            };
        }
        let mut agents = agents;
        for agent in &mut agents {
            let id = agent.id.clone();
            agent.thread.rekey_to_agent(&id);
            if agent.conversation_id.is_none() {
                agent.conversation_id = Some(id);
            }
            if agent.settings_version < CURRENT_SETTINGS_VERSION {
                // Before settings were agent-owned, same-provider agents spent
                // the entity selection while cross-provider agents kept their
                // own vocabulary. Freeze precisely that effective answer.
                if agent.choice.provider == choice.provider {
                    agent.choice = choice.clone();
                }
                agent.settings_version = CURRENT_SETTINGS_VERSION;
            }
        }
        AgentRoster { agents }
    }

    /// Who [`restore`](Self::restore) would say this record's agents are —
    /// id, name, rail position, provider — without cloning a conversation.
    ///
    /// Restoring moves the entity's model choice onto an agent that predates
    /// agent-owned settings only where the providers already agree, so the
    /// provider an agent had stored is the one it restores with.
    pub fn restored_members(
        owner_id: &str,
        agents: &[Agent],
        legacy: &Thread,
        choice: &ModelChoice,
    ) -> Vec<RosterMember> {
        if agents.is_empty() && !legacy.is_empty() {
            return vec![RosterMember {
                id: derived_agent_id(owner_id),
                name: None,
                ordinal: 1,
                provider: choice.provider,
            }];
        }
        agents
            .iter()
            .map(|agent| RosterMember {
                id: agent.id.clone(),
                name: agent.name.clone(),
                ordinal: agent.ordinal,
                provider: agent.choice.provider,
            })
            .collect()
    }

    /// The agent at index 0 — the one entity-level events speak to and the one
    /// a verb that names no agent means. `None` on an agentless entity, where
    /// there is nobody to tell and nothing to address.
    pub fn primary(&self) -> Option<&Agent> {
        self.agents.first()
    }

    pub fn primary_mut(&mut self) -> Option<&mut Agent> {
        self.agents.first_mut()
    }

    /// Whether `agent_id` is the primary — the agent whose conversation is the
    /// entity's own, and the one every verb that names no agent reaches. False
    /// on an agentless roster: nobody speaks for it.
    pub fn is_primary(&self, agent_id: &str) -> bool {
        self.primary().is_some_and(|primary| primary.id == agent_id)
    }

    /// The one agent of a task. A task is created with its agent and can
    /// neither gain nor lose one, so this holds by construction — named once
    /// here rather than spelled out at every plan site.
    pub fn sole(&self) -> &Agent {
        self.primary().expect(TASK_HOLDS_ITS_ONE_AGENT)
    }

    pub fn sole_mut(&mut self) -> &mut Agent {
        self.primary_mut().expect(TASK_HOLDS_ITS_ONE_AGENT)
    }

    /// A task's conversation. Almost every caller of [`sole`](Self::sole)
    /// wants the thread rather than the agent around it, so the walk is named
    /// once here instead of being spelled out at each of them.
    pub fn sole_thread(&self) -> &Thread {
        &self.sole().thread
    }

    pub fn sole_thread_mut(&mut self) -> &mut Thread {
        &mut self.sole_mut().thread
    }

    /// The agent the system delivers to: the primary, or a freshly minted one
    /// when the human has left nobody here.
    ///
    /// The one door for every path that MUST be heard — a post, a start, a
    /// dispatched turn, a report coming back. `choice` is the entity's own, so
    /// the agent Build mints runs what the entity was set up to run.
    pub fn ensure_primary<'a>(
        &mut self,
        owner: impl Into<AgentOwner<'a>>,
        choice: ModelChoice,
        now: &str,
    ) -> &mut Agent {
        if self.agents.is_empty() {
            self.add(owner, choice, now);
        }
        self.primary_mut().expect("just ensured")
    }

    /// What a turn addressed to `agent_id` spends. Settings belong to the agent
    /// on every provider, and an invalid address never falls back to a sibling.
    pub fn turn_choice(&self, agent_id: &str) -> Result<ModelChoice, String> {
        self.by_id(agent_id)
            .map(|agent| agent.choice.clone())
            .ok_or_else(|| format!("unknown agent_id: {agent_id}"))
    }

    pub fn agents(&self) -> &[Agent] {
        &self.agents
    }

    pub fn iter(&self) -> std::slice::Iter<'_, Agent> {
        self.agents.iter()
    }

    pub fn iter_mut(&mut self) -> std::slice::IterMut<'_, Agent> {
        self.agents.iter_mut()
    }

    pub fn len(&self) -> usize {
        self.agents.len()
    }

    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
    }

    pub fn by_id(&self, agent_id: &str) -> Option<&Agent> {
        self.agents.iter().find(|agent| agent.id == agent_id)
    }

    pub fn by_id_mut(&mut self, agent_id: &str) -> Option<&mut Agent> {
        self.agents.iter_mut().find(|agent| agent.id == agent_id)
    }

    /// Which agent a verb means: the one it named, or the primary — so every
    /// verb that predates agents keeps addressing the conversation it always
    /// did. A read of an agentless entity is refused rather than answered with
    /// somebody else's conversation; the paths that must be heard mint one
    /// instead, through [`ensure_primary`](AgentRoster::ensure_primary).
    pub fn resolve(&self, agent_id: Option<&str>) -> Result<&Agent, String> {
        match agent_id {
            None => self.primary().ok_or_else(|| NO_AGENT_YET.to_string()),
            Some("") => Err("agent_id cannot be empty".to_string()),
            Some(id) => self
                .by_id(id)
                .ok_or_else(|| format!("unknown agent_id: {id}")),
        }
    }

    pub fn resolve_mut(&mut self, agent_id: Option<&str>) -> Result<&mut Agent, String> {
        match agent_id {
            None => self.primary_mut().ok_or_else(|| NO_AGENT_YET.to_string()),
            Some("") => Err("agent_id cannot be empty".to_string()),
            Some(id) => {
                if self.by_id(id).is_none() {
                    return Err(format!("unknown agent_id: {id}"));
                }
                Ok(self.by_id_mut(id).expect("just checked"))
            }
        }
    }

    /// Add an agent to the entity, with its own empty conversation. The ordinal
    /// continues past the highest one ever handed out here, so a rail label is
    /// never reused.
    pub fn add<'a>(
        &mut self,
        owner: impl Into<AgentOwner<'a>>,
        choice: ModelChoice,
        now: &str,
    ) -> &Agent {
        let owner = owner.into();
        let ordinal = self
            .agents
            .iter()
            .map(|agent| agent.ordinal)
            .max()
            .unwrap_or(0)
            + 1;
        self.agents.push(Agent::new(
            owner.kind.new_id(),
            owner.id,
            choice,
            ordinal,
            now,
        ));
        self.agents.last().expect("just pushed")
    }

    /// Add once for a client-supplied creation operation. Retrying the exact
    /// operation returns the same durable agent; reusing its id for different
    /// settings is refused instead of ambiguously creating or mutating one.
    pub fn add_idempotent<'a>(
        &mut self,
        owner: impl Into<AgentOwner<'a>>,
        choice: ModelChoice,
        now: &str,
        creation_id: &str,
    ) -> Result<(Agent, bool), String> {
        if creation_id.is_empty() {
            return Err("creation_id cannot be empty".to_string());
        }
        if let Some(index) = self
            .agents
            .iter()
            .position(|agent| agent.creation_id.as_deref() == Some(creation_id))
        {
            let existing = &self.agents[index];
            if existing.creation_choice.as_ref() != Some(&choice) {
                return Err(format!(
                    "creation_id {creation_id} was already used with different agent settings"
                ));
            }
            return Ok((existing.clone(), false));
        }
        let added_id = self.add(owner, choice.clone(), now).id.clone();
        let added = self.by_id_mut(&added_id).expect("the agent was just added");
        added.creation_id = Some(creation_id.to_string());
        added.creation_choice = Some(choice);
        Ok((
            self.by_id(&added_id)
                .expect("the agent was just added")
                .clone(),
            true,
        ))
    }

    /// Take an agent off the entity, handing back the record that was removed
    /// (its conversation included, for the caller to do the last rites on).
    ///
    /// Any agent, the primary and the last one included: a branch with no
    /// agents is a working branch that shows the new-agent view, and the next
    /// thing the system has to say to it mints one.
    ///
    /// The ordinals of the agents beside it are left alone. An ordinal is the
    /// rail's label for an agent, not its position, and a label that shifted
    /// when a neighbour went away would rename a conversation the human is in
    /// the middle of reading.
    pub fn remove(&mut self, agent_id: &str) -> Result<Agent, String> {
        let index = self
            .agents
            .iter()
            .position(|agent| agent.id == agent_id)
            .ok_or_else(|| format!("unknown agent_id: {agent_id}"))?;
        Ok(self.agents.remove(index))
    }
}

/// What a task's roster holds, named where the unwrap that relies on it is.
const TASK_HOLDS_ITS_ONE_AGENT: &str = "a task always holds its one agent";

/// What a read of an agentless entity is told.
const NO_AGENT_YET: &str = "no agent here yet — send a message to create one";

/// A fresh, time-ordered agent id. Two agents minted in the same millisecond
/// still differ (80 bits of randomness), and ids minted later sort later.
pub fn new_agent_id() -> String {
    mint_agent_id(now_ms(), uuid::Uuid::new_v4().as_u128())
}

/// A fresh project-agent id: the same time-ordered body under the prefix that
/// names the project surface.
pub fn new_project_agent_id() -> String {
    format!("{PROJECT_AGENT_ID_PREFIX}{}", new_ulid_body())
}

/// A fresh time-ordered id body, prefix-free: 48 bits of milliseconds then 80
/// bits of randomness, Crockford base32.
///
/// Exposed because the ULID rule is the house rule for every minted id, and a
/// record that is not an agent — a tracker task, its comments, its events —
/// wants the same sortable body under its own prefix rather than a second
/// spelling of the same idea.
pub fn new_ulid_body() -> String {
    ulid_body(now_ms(), uuid::Uuid::new_v4().as_u128())
}

/// Milliseconds since the epoch, as a ULID's time half reads them.
pub(crate) fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis())
        .unwrap_or(0)
}

/// The ULID rule, factored out so time and randomness can be supplied by a
/// test: 48 bits of milliseconds, then 80 bits of randomness.
pub fn mint_agent_id(now_ms: u128, randomness: u128) -> String {
    format!("{AGENT_ID_PREFIX}{}", ulid_body(now_ms, randomness))
}

/// The ULID body of an exact reading, for a mint that supplies its own —
/// a record whose ids must sort in the order they were minted rather than
/// only to the millisecond (`crate::tracker`).
pub(crate) fn ulid_body_of(now_ms: u128, randomness: u128) -> String {
    ulid_body(now_ms, randomness)
}

fn ulid_body(now_ms: u128, randomness: u128) -> String {
    let time = (now_ms & ((1u128 << 48) - 1)) << 80;
    let random = randomness & ((1u128 << 80) - 1);
    crockford_base32(time | random)
}

/// The first agent of `owner_id`, derived rather than minted.
///
/// A boot migration has to name the agent it is moving an entity's existing
/// conversation onto, and has to name the SAME one if it runs again. Deriving
/// that id from the owner is what makes re-migration a no-op instead of a
/// second agent holding a second copy of the conversation.
pub fn derived_agent_id(owner_id: &str) -> String {
    let digest = Sha256::digest(format!("build-agent-1:{owner_id}").as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    format!(
        "{AGENT_ID_PREFIX}{}",
        crockford_base32(u128::from_be_bytes(bytes))
    )
}

/// The roster a stored record carries, for a test that needs one on a record
/// it builds by hand. One agent, its id derived from its owner the way the
/// pre-agent migration named it — so a tab key derived from the owner finds
/// its session — on the harness defaults.
///
/// Records are assembled in both `app`'s and `orchestrator`'s tests, which is
/// why this stands here rather than in either of them.
#[cfg(test)]
pub fn stored_agents(owner_id: &str) -> Vec<Agent> {
    vec![Agent::new(
        derived_agent_id(owner_id),
        owner_id,
        ModelChoice::default(),
        1,
        "2026-07-01T10:00:00Z",
    )]
}

/// 128 bits as 26 Crockford base32 characters, most significant first.
fn crockford_base32(value: u128) -> String {
    let mut out = [b'0'; ULID_LEN];
    let mut remaining = value;
    for slot in out.iter_mut().rev() {
        *slot = CROCKFORD[(remaining & 0x1f) as usize];
        remaining >>= 5;
    }
    String::from_utf8(out.to_vec()).expect("Crockford digits are ASCII")
}

#[cfg(test)]
mod id_tests {
    use super::*;

    fn body(id: &str) -> &str {
        id.strip_prefix(AGENT_ID_PREFIX)
            .unwrap_or_else(|| panic!("{id} is not an agent id"))
    }

    #[test]
    fn an_agent_id_is_a_prefixed_ulid() {
        let id = new_agent_id();
        let body = body(&id);
        assert_eq!(body.len(), ULID_LEN, "{id}");
        assert!(
            body.bytes().all(|c| CROCKFORD.contains(&c)),
            "{id} uses characters outside Crockford base32"
        );
    }

    /// The rail orders agents by when they were added, and a durable id that
    /// sorts by mint time means that ordering survives without a join.
    #[test]
    fn ids_minted_later_sort_later() {
        let earlier = mint_agent_id(1_760_000_000_000, u128::MAX);
        let later = mint_agent_id(1_760_000_000_001, 0);
        assert!(earlier < later, "{earlier} should sort before {later}");
    }

    #[test]
    fn two_ids_from_the_same_millisecond_still_differ() {
        let one = mint_agent_id(1_760_000_000_000, 1);
        let two = mint_agent_id(1_760_000_000_000, 2);
        assert_ne!(one, two);
    }

    /// The whole point of deriving: the migration can run twice and name the
    /// same agent both times.
    #[test]
    fn a_derived_id_is_stable_for_one_owner_and_unique_across_owners() {
        assert_eq!(derived_agent_id("run-1"), derived_agent_id("run-1"));
        assert_ne!(derived_agent_id("run-1"), derived_agent_id("run-2"));
        assert_eq!(body(&derived_agent_id("run-1")).len(), ULID_LEN);
        assert!(body(&derived_agent_id("run-1"))
            .bytes()
            .all(|c| CROCKFORD.contains(&c)));
    }
}

#[cfg(test)]
mod roster_tests {
    use super::*;
    use crate::models::AgentProvider;

    const NOW: &str = "2026-08-13T09:00:00Z";

    fn codex() -> ModelChoice {
        ModelChoice {
            provider: AgentProvider::Codex,
            model: Some("gpt-5.6-sol".into()),
            effort: Some("high".into()),
        }
    }

    #[test]
    fn the_first_agent_owns_the_entitys_conversation() {
        let roster = AgentRoster::with_first("run-1", ModelChoice::default(), NOW);
        let first = roster.primary().expect("the entity's agent");
        assert_eq!(first.ordinal, 1);
        assert_eq!(first.owner_id, "run-1");
        assert_eq!(first.state, AgentLifecycle::Idle);
        assert_eq!(first.thread.id, format!("thread:{}", first.id));
        assert_eq!(first.thread.agent.id, first.id);
        assert_eq!(first.conversation_id(), first.id);
        assert_eq!(first.settings_version, CURRENT_SETTINGS_VERSION);
        // The roster IS that conversation to everything that has no agent of
        // its own to speak to.
        assert_eq!(roster.primary().unwrap().thread.id, first.thread.id);
    }

    #[test]
    fn a_second_agent_gets_its_own_conversation_and_the_next_ordinal() {
        let mut roster = AgentRoster::with_first("run-1", ModelChoice::default(), NOW);
        let first_id = roster.primary().unwrap().id.clone();
        let added = roster.add("run-1", codex(), "2026-08-13T10:00:00Z").clone();

        assert_eq!(added.ordinal, 2);
        assert_eq!(added.choice, codex());
        assert_ne!(added.id, first_id);
        assert_eq!(added.thread.id, format!("thread:{}", added.id));
        assert_eq!(roster.len(), 2);

        // Two conversations, not one shared one.
        roster
            .by_id_mut(&added.id)
            .expect("the agent is on the roster")
            .thread
            .post_user("only agent two hears this", None, NOW);
        assert_eq!(roster.primary().unwrap().thread.items.len(), 0);
        assert_eq!(roster.by_id(&added.id).unwrap().thread.items.len(), 1);
    }

    #[test]
    fn resolve_defaults_to_the_first_agent_and_rejects_invalid_explicit_ids() {
        let mut roster = AgentRoster::with_first("run-1", ModelChoice::default(), NOW);
        let second = roster.add("run-1", ModelChoice::default(), NOW).id.clone();

        assert_eq!(roster.resolve(None).unwrap().ordinal, 1);
        assert!(roster.resolve(Some("")).unwrap_err().contains("empty"));
        assert_eq!(roster.resolve(Some(&second)).unwrap().ordinal, 2);
        assert!(roster
            .resolve(Some("agent-NOPE"))
            .unwrap_err()
            .contains("unknown agent_id"));
    }

    #[test]
    fn restoring_legacy_settings_freezes_each_agents_effective_choice() {
        let entity_choice = codex();
        let mut same_provider = Agent::new(
            "agent-same",
            "run-1",
            ModelChoice {
                provider: AgentProvider::Codex,
                model: Some("stale-model".into()),
                effort: None,
            },
            1,
            NOW,
        );
        same_provider.settings_version = 0;
        same_provider.conversation_id = None;
        let mut other_provider = Agent::new("agent-other", "run-1", ModelChoice::default(), 2, NOW);
        other_provider.settings_version = 0;
        other_provider.conversation_id = None;

        let roster = AgentRoster::restore(
            "run-1",
            vec![same_provider, other_provider],
            Thread::default(),
            entity_choice.clone(),
            NOW,
        );

        let same = roster.by_id("agent-same").unwrap();
        assert_eq!(same.choice, entity_choice);
        assert_eq!(same.conversation_id(), "agent-same");
        assert_eq!(same.settings_version, CURRENT_SETTINGS_VERSION);
        let other = roster.by_id("agent-other").unwrap();
        assert_eq!(other.choice.provider, AgentProvider::Claude);
        assert_eq!(other.conversation_id(), "agent-other");
        assert_eq!(other.settings_version, CURRENT_SETTINGS_VERSION);
    }

    #[test]
    fn choosing_is_agent_owned_and_advances_a_monotonic_revision() {
        let mut agent = Agent::new("agent-one", "run-1", ModelChoice::default(), 1, NOW);

        assert_eq!(agent.choose(codex()), 1);
        assert_eq!(agent.choice, codex());
        assert_eq!(agent.choose(ModelChoice::default()), 2);
        assert_eq!(agent.choice_revision, 2);
    }

    #[test]
    fn a_conversation_binding_does_not_follow_roster_position() {
        let mut roster = AgentRoster::with_first("run-1", ModelChoice::default(), NOW);
        let first = roster.primary().unwrap().id.clone();
        let second = roster.add("run-1", codex(), NOW).id.clone();
        roster
            .by_id_mut(&first)
            .unwrap()
            .bind_conversation("agent-task");

        roster.remove(&first).unwrap();

        assert_eq!(roster.primary().unwrap().id, second);
        assert_eq!(roster.primary().unwrap().conversation_id(), second);
    }

    #[test]
    fn a_creation_operation_is_idempotent_and_cannot_be_reused_differently() {
        let mut roster = AgentRoster::empty();
        let (created, was_created) = roster
            .add_idempotent("run-1", codex(), NOW, "create-1")
            .unwrap();
        let id = created.id.clone();
        assert!(was_created);

        let (retried, was_created) = roster
            .add_idempotent("run-1", codex(), NOW, "create-1")
            .unwrap();
        assert_eq!(retried.id, id);
        assert!(!was_created);
        assert_eq!(roster.len(), 1);

        let refused = roster
            .add_idempotent("run-1", ModelChoice::default(), NOW, "create-1")
            .unwrap_err();
        assert!(refused.contains("different agent settings"), "{refused}");
        assert_eq!(roster.len(), 1);
    }

    /// The migration contract: an entity's conversation becomes its first
    /// agent's, keeps every item, and re-running the restore changes nothing.
    #[test]
    fn restoring_a_pre_agent_record_adopts_its_conversation_idempotently() {
        let mut legacy = Thread::new("run-1");
        legacy.post_user("carried across", None, NOW);
        legacy.post_agent("and answered", None, NOW);

        let roster = AgentRoster::restore(
            "run-1",
            Vec::new(),
            legacy.clone(),
            ModelChoice::default(),
            NOW,
        );
        let first = roster.primary().expect("the migrated agent");
        assert_eq!(first.id, derived_agent_id("run-1"));
        assert_eq!(first.thread.id, format!("thread:{}", first.id));
        assert_eq!(first.thread.agent.id, first.id);
        assert_eq!(first.thread.items, legacy.items);

        let again = AgentRoster::restore(
            "run-1",
            roster.agents().to_vec(),
            Thread::default(),
            ModelChoice::default(),
            NOW,
        );
        assert_eq!(again, roster, "a second migration is a no-op");
        assert_eq!(again.len(), 1);
    }

    /// The migration exists for a conversation written before agents did.
    /// A record with no conversation to carry across and no agents left is an
    /// entity whose agents were removed — it restores as what it is.
    #[test]
    fn a_record_with_nothing_to_migrate_restores_empty() {
        let roster = AgentRoster::restore(
            "run-1",
            Vec::new(),
            Thread::default(),
            ModelChoice::default(),
            NOW,
        );
        assert!(roster.is_empty());
        assert_eq!(roster.agents(), &[]);
    }

    /// Removing an agent takes it off the rail and leaves every other label
    /// exactly where it was — the ordinal is a name, not a position.
    #[test]
    fn remove_takes_one_agent_off_without_renumbering_the_others() {
        let mut roster = AgentRoster::with_first("run-1", ModelChoice::default(), NOW);
        let first = roster.primary().unwrap().id.clone();
        let second = roster.add("run-1", codex(), NOW).id.clone();
        let third = roster.add("run-1", ModelChoice::default(), NOW).id.clone();

        let removed = roster
            .remove(&second)
            .expect("the second agent is removable");
        assert_eq!(removed.id, second);
        assert_eq!(removed.ordinal, 2);
        assert_eq!(roster.len(), 2);
        assert!(roster.by_id(&second).is_none());
        assert_eq!(roster.primary().unwrap().id, first);
        assert_eq!(roster.by_id(&third).expect("still here").ordinal, 3);

        // The next agent continues past the highest ordinal ever handed out
        // here, so the freed label is never reused.
        assert_eq!(roster.add("run-1", ModelChoice::default(), NOW).ordinal, 4);
    }

    /// Every agent is removable, the primary and the last one included. A
    /// branch with no agents is a working branch: it shows the new-agent view,
    /// and the next thing said to it creates one.
    #[test]
    fn every_agent_is_removable_down_to_none() {
        let mut roster = AgentRoster::with_first("run-1", ModelChoice::default(), NOW);
        let first = roster.primary().expect("just created").id.clone();
        let second = roster.add("run-1", codex(), NOW).id.clone();

        assert_eq!(roster.remove(&first).expect("removable").id, first);
        assert_eq!(
            roster.primary().expect("the second agent is now first").id,
            second
        );

        roster.remove(&second).expect("the last one goes too");
        assert!(roster.is_empty());
        assert_eq!(roster.primary(), None);
        assert!(roster.resolve(None).unwrap_err().contains("no agent"));
    }

    /// The door every path takes when the system must be heard: the agent at
    /// index 0, or one minted on the entity's own choice when the human has
    /// left nobody there.
    #[test]
    fn ensure_primary_answers_index_zero_or_mints_one() {
        let mut roster = AgentRoster::empty();
        let minted = roster
            .ensure_primary("run-1", codex(), "2026-08-30T09:00:00Z")
            .clone();
        assert_eq!(minted.choice, codex());
        assert_eq!(minted.ordinal, 1);
        assert_eq!(roster.len(), 1);

        let second = roster.add("run-1", ModelChoice::default(), NOW).id.clone();
        assert_eq!(
            roster.ensure_primary("run-1", codex(), NOW).id,
            minted.id,
            "an occupied roster mints nobody"
        );
        assert_eq!(roster.len(), 2);
        assert_ne!(second, minted.id);
    }

    /// The prefix says which MCP surface a session gets, so it is the OWNER
    /// that decides it: a project's conversation owner mints project agents,
    /// and everything else mints coding ones.
    #[test]
    fn a_project_owner_mints_project_agents_and_every_other_owner_coding_ones() {
        let mut project = AgentRoster::empty();
        let first = project
            .ensure_primary(AgentOwner::project("run-project"), codex(), NOW)
            .id
            .clone();
        let second = project
            .add(AgentOwner::project("run-project"), codex(), NOW)
            .id
            .clone();
        for id in [&first, &second] {
            assert!(is_project_agent(id), "{id}");
            assert!(!crate::router::is_router_agent(id), "{id}");
        }
        assert_ne!(first, second);

        let mut branch = AgentRoster::empty();
        let coding = branch.add("run-1", codex(), NOW).id.clone();
        assert!(coding.starts_with(AGENT_ID_PREFIX), "{coding}");
        assert!(!is_project_agent(&coding), "{coding}");
    }

    /// A minted agent is minted, first or not: only the pre-agent migration
    /// derives an id, and it is the only thing that may.
    #[test]
    fn an_agent_added_to_an_empty_roster_is_minted_not_derived() {
        let mut roster = AgentRoster::empty();
        let added = roster.add("run-1", ModelChoice::default(), NOW).id.clone();
        assert_ne!(added, derived_agent_id("run-1"));
        assert_eq!(roster.primary().expect("just added").id, added);
    }

    /// Model and effort are agent-owned even when the entity happens to use the
    /// same provider.
    #[test]
    fn a_turn_spends_the_agents_own_choice() {
        let roster = AgentRoster::with_first("run-1", ModelChoice::default(), NOW);
        let first = roster.primary().expect("the entity's agent").id.clone();

        assert_eq!(roster.turn_choice(&first).unwrap(), ModelChoice::default());
    }

    /// An agent that runs another harness than the entity keeps its own
    /// selection whole: a model id written in one harness's vocabulary names
    /// nothing in another's, so the entity's is not spent here.
    #[test]
    fn an_agent_on_another_harness_keeps_its_own_model() {
        let mut roster = AgentRoster::with_first("run-1", ModelChoice::default(), NOW);
        let codex_agent = roster.add("run-1", codex(), NOW).id.clone();
        assert_eq!(roster.turn_choice(&codex_agent).unwrap(), codex());
    }

    /// Nobody by that name: no sibling or entity choice may be spent instead.
    #[test]
    fn a_turn_addressed_to_no_one_is_refused() {
        let roster = AgentRoster::with_first("run-1", ModelChoice::default(), NOW);
        assert!(roster
            .turn_choice("agent-NOPE")
            .unwrap_err()
            .contains("unknown agent_id"));
    }

    #[test]
    fn removing_an_agent_that_is_not_on_the_roster_is_an_error() {
        let mut roster = AgentRoster::with_first("run-1", ModelChoice::default(), NOW);
        roster.add("run-1", codex(), NOW);
        assert!(roster
            .remove("agent-NOPE")
            .unwrap_err()
            .contains("unknown agent_id"));
        assert_eq!(roster.len(), 2);
    }

    #[test]
    fn a_roster_round_trips_as_a_plain_array() {
        let mut roster = AgentRoster::with_first("run-1", ModelChoice::default(), NOW);
        roster.add("run-1", codex(), NOW);
        let wire = serde_json::to_value(&roster).expect("a roster serializes");
        assert!(wire.is_array(), "{wire:?}");
        assert_eq!(wire[1]["ordinal"], 2);
        assert_eq!(wire[1]["state"], "idle");
        assert_eq!(
            serde_json::from_value::<AgentRoster>(wire).expect("a roster reloads"),
            roster
        );
    }
}

#[cfg(test)]
mod name_tests {
    use super::*;

    /// A name is what a rail bubble and a notice line wear, so it is trimmed
    /// and its inner spacing collapsed — otherwise "Rail  scroll" and "Rail
    /// scroll" are two names for one agent and a reader cannot tell them apart.
    #[test]
    fn a_name_is_stored_as_it_will_be_read() {
        assert_eq!(agent_name_from("  Rail   scroll  ").unwrap(), "Rail scroll");
        assert_eq!(agent_name_from("Tracker").unwrap(), "Tracker");
        assert_eq!(agent_name_from("\tTransport\n").unwrap(), "Transport");
    }

    /// Every refusal is a sentence the user could read, because it is one an
    /// agent repeats to them.
    #[test]
    fn a_name_that_will_not_fit_is_refused_in_a_sentence() {
        for (word, expected) in [
            ("", "at least 2 characters"),
            ("  ", "at least 2 characters"),
            ("x", "at least 2 characters"),
            (
                "Rail scroll and the composer clearance",
                "at most 24 characters",
            ),
            ("one two three four", "one or two words"),
        ] {
            let refusal = agent_name_from(word).expect_err(word);
            assert!(refusal.contains(expected), "{word}: {refusal}");
            assert!(refusal.ends_with('.'), "reads as a sentence: {refusal}");
            assert!(
                refusal.starts_with("An agent's name"),
                "names the thing rather than the verb: {refusal}"
            );
        }
    }

    /// Three words is the cap rather than two: "Rail scroll fix" is a name
    /// somebody would reasonably choose, and refusing it would be refusing
    /// the shape of the thing being asked for.
    #[test]
    fn three_words_fit_and_four_do_not() {
        assert_eq!(
            agent_name_from("Rail scroll fix").unwrap(),
            "Rail scroll fix"
        );
        assert!(agent_name_from("Rail scroll fix now").is_err());
    }

    /// Case and spacing do not make a second agent.
    #[test]
    fn the_same_name_said_differently_is_the_same_name() {
        assert!(same_agent_name("Rail scroll", "rail  scroll"));
        assert!(same_agent_name("Tracker", "TRACKER"));
        assert!(!same_agent_name("Tracker", "Trackers"));
        assert!(!same_agent_name("Rail scroll", "Rail"));
    }

    /// The two fields are absent on the wire until they are set, so a client
    /// that has never heard of either reads an agent exactly as it did.
    #[test]
    fn an_unnamed_agent_carries_neither_field() {
        let agent = Agent::new(
            "agent-1",
            "run-1",
            ModelChoice::default(),
            1,
            "2026-09-20T21:00:00Z",
        );
        let wire = serde_json::to_value(&agent).unwrap();
        assert!(wire.get("name").is_none(), "{wire:?}");
        assert!(wire.get("name_asked").is_none(), "{wire:?}");
    }
}

#[cfg(test)]
mod context_tests {
    use super::*;

    fn with_context(tokens: Option<u64>, max_context_tokens: Option<u64>) -> Agent {
        let mut agent = Agent::new(
            "agent-1",
            "run-1",
            ModelChoice::default(),
            1,
            "2026-09-21T09:00:00Z",
        );
        agent.last_context_tokens = tokens;
        agent.max_context_tokens = max_context_tokens;
        agent
    }

    #[test]
    fn compaction_is_due_at_the_threshold_and_not_below_it() {
        assert!(with_context(Some(200_000), None).compaction_due(200_000));
        assert!(with_context(Some(250_000), None).compaction_due(200_000));
        assert!(!with_context(Some(199_999), None).compaction_due(200_000));
        assert!(
            !with_context(None, None).compaction_due(200_000),
            "an agent whose context nobody has reported is never compacted"
        );
    }

    #[test]
    fn the_agents_own_limit_beats_the_device_and_zero_is_never() {
        let tighter = with_context(Some(60_000), Some(50_000));
        assert_eq!(tighter.compact_at_tokens(200_000), 50_000);
        assert!(tighter.compaction_due(200_000));

        let never = with_context(Some(900_000), Some(0));
        assert_eq!(never.compact_at_tokens(200_000), 0);
        assert!(!never.compaction_due(200_000));

        let device_off = with_context(Some(900_000), None);
        assert_eq!(device_off.compact_at_tokens(0), 0);
        assert!(!device_off.compaction_due(0));
    }

    #[test]
    fn a_reading_carries_the_tokens_the_time_and_the_running_models_window() {
        let mut agent = with_context(Some(612_000), None);
        agent.last_context_at = Some("2026-09-21T09:30:00Z".to_string());
        agent.active_model = Some("claude-opus-5[1m]".to_string());
        assert_eq!(
            agent.context_reading(0),
            Some(crate::thread::ContextReading {
                tokens: 612_000,
                window: Some(1_000_000),
                compact_at: None,
                at: "2026-09-21T09:30:00Z".to_string(),
            })
        );
        agent.active_model = None;
        assert_eq!(
            agent.context_reading(0).and_then(|reading| reading.window),
            None,
            "a default model is no model whose window Build knows"
        );
    }

    /// Where the chat compacts rides the reading: the chat's own limit, else
    /// the device's, and none when neither compacts.
    #[test]
    fn a_reading_carries_where_the_chat_compacts() {
        let mut agent = with_context(Some(190_000), None);
        agent.last_context_at = Some("2026-09-21T09:30:00Z".to_string());
        let compact_at = |agent: &Agent, device| {
            agent
                .context_reading(device)
                .and_then(|reading| reading.compact_at)
        };
        assert_eq!(compact_at(&agent, 200_000), Some(200_000));
        assert_eq!(compact_at(&agent, 0), None);
        agent.max_context_tokens = Some(150_000);
        assert_eq!(compact_at(&agent, 200_000), Some(150_000));
        agent.max_context_tokens = Some(0);
        assert_eq!(compact_at(&agent, 200_000), None);
    }

    #[test]
    fn there_is_no_reading_without_tokens_or_a_time() {
        let mut agent = with_context(None, None);
        agent.last_context_at = Some("2026-09-21T09:30:00Z".to_string());
        assert_eq!(agent.context_reading(200_000), None);
        let untimed = with_context(Some(612_000), None);
        assert_eq!(untimed.context_reading(200_000), None);
    }

    #[test]
    fn a_record_written_before_context_was_tracked_loads_without_it() {
        let mut wire = serde_json::to_value(Agent::new(
            "agent-1",
            "run-1",
            ModelChoice::default(),
            1,
            "2026-09-21T09:00:00Z",
        ))
        .unwrap();
        for field in [
            "last_context_tokens",
            "session_cache_read_tokens",
            "max_context_tokens",
            "last_context_at",
        ] {
            wire.as_object_mut().unwrap().remove(field);
        }
        let agent: Agent = serde_json::from_value(wire).unwrap();
        assert_eq!(agent.last_context_tokens, None);
        assert_eq!(agent.session_cache_read_tokens, None);
        assert_eq!(agent.max_context_tokens, None);
    }
}
