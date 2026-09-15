use crate::app::{AppState, SettlingHandle, Spawned, TabKey};
use crate::models::ModelChoice;
use crate::operation::OperationReceipt;
use crate::orchestrator::{ActivePlan, ActiveRun, AgentTurn};
use crate::store::now_rfc3339;
use crate::timing::FrameClock;
use std::sync::Arc;

/// An [`AgentTurn`] addressed to a worktree, waiting for the state lock to be
/// free.
///
/// Every lifecycle verb runs inside `state.lock().unwrap().dispatch(..)`, and
/// [`deliver`] takes that same lock and holds nothing while it blocks for
/// seconds spawning a cold harness. So a verb records what it wants said and
/// [`dispatch_frame`] — which holds the `Arc` and no guard — sends it the moment
/// the verb returns.
pub(in crate::app) struct PendingAgentTurn {
    /// Durable reviewer operation this turn delivers. Lifecycle turns predate
    /// operation receipts and carry none.
    pub(in crate::app) operation_id: Option<String>,
    /// The worktree the agent that hears this turn works in. Canonical at
    /// construction — every site that builds a turn passes it through
    /// `AppState::canonical_root` — so [`Self::tab_key`] is a field read and
    /// makes no filesystem call under the app mutex.
    pub(in crate::app) root: std::path::PathBuf,
    /// The plan/run whose lifecycle this turn moves.
    pub(in crate::app) owner: String,
    /// The agent that hears it. Entity-level work addresses the entity's first
    /// agent; a verb the rail addressed names the agent whose bubble was open.
    pub(in crate::app) agent_id: String,
    /// Canonical conversation binding captured with the addressed agent. A
    /// queued turn must not follow an agent id after that binding was removed
    /// or changed while the delivery sat outside the app lock.
    pub(in crate::app) conversation_id: String,
    pub(in crate::app) model_choice: ModelChoice,
    pub(in crate::app) choice_revision: u64,
    pub(in crate::app) interrupt: bool,
    /// What to say once the tab is open — `None` for a turn that only wants
    /// the agent there.
    ///
    /// "Start this agent" and "tell this agent something" are one job with one
    /// queue: the tab has to exist either way, and the spawn is the same spawn.
    /// The difference is whether anything is written into it afterwards.
    pub(in crate::app) say: Option<TurnText>,
    /// The phase recorded on the conversation's session lineage if the turn
    /// turns out to be cold — a cold delivery is a new agent process.
    pub(in crate::app) phase: &'static str,
    /// Whether `cold` is closed with the durable conversation — the catch-up
    /// packet and the previous completion report — when the turn is handed
    /// over.
    ///
    /// True for every turn addressed to an entity's conversation, false for
    /// the router's: a router is one decision long, works no conversation, and
    /// its prompt deliberately carries none.
    pub(in crate::app) wants_catch_up: bool,
    /// Whether this turn outlives a refusal of the request that queued it.
    ///
    /// False for almost everything: a turn speaks for a mutation, and a request
    /// that failed wrote no mutation to speak for. A recovery is the exception
    /// — it is written down and started, and the verb then refuses its caller
    /// to say exactly that, so the agent handed the recovery must still hear
    /// it.
    pub(in crate::app) survives_refusal: bool,
}

impl PendingAgentTurn {
    pub(in crate::app) fn for_delivery_operation(receipt: &OperationReceipt) -> Option<Self> {
        let delivery = receipt.delivery.as_ref()?;
        let payload = delivery.payload.as_ref()?;
        Some(Self {
            operation_id: Some(receipt.operation_id.clone()),
            root: AppState::canonical_root(&delivery.root),
            owner: delivery.owner_id.clone(),
            agent_id: delivery.agent_id.clone(),
            conversation_id: receipt.conversation_id.clone(),
            model_choice: delivery.model_choice.clone(),
            choice_revision: delivery.choice_revision,
            interrupt: delivery.interrupt,
            say: Some(TurnText {
                cold: payload.delivery_prompt(&receipt.operation_id, true),
                warm: payload.delivery_prompt(&receipt.operation_id, false),
            }),
            phase: "revive",
            wants_catch_up: false,
            survives_refusal: true,
        })
    }

    /// What this turn says, for a test that queued one that says something.
    #[cfg(test)]
    pub(in crate::app) fn said(&self) -> &TurnText {
        self.say.as_ref().expect("this turn carries text")
    }

    /// Whether the agent is TOLD anything once the tab is open. A turn that
    /// says nothing opens a harness and sends it nothing, so it promises the
    /// agent nothing to read.
    pub(in crate::app) fn says_something(&self) -> bool {
        self.say.is_some()
    }

    /// The registry entry this turn is on its way to. The same key
    /// [`ensure_agent_tab`] will reserve, so a turn in the queue, a turn
    /// mid-delivery and a spawn in flight are all one agent's under one name.
    pub(in crate::app) fn tab_key(&self) -> TabKey {
        TabKey::agent(&self.root, &self.agent_id)
    }

    /// Address a run's turn to the run's worktree. Canonical, because the same
    /// worktree reaches the tab registry under several scope shapes.
    ///
    /// The turn goes to the run's primary agent, and mints one on a roster the
    /// human emptied: this is Build about to run something, and something Build
    /// runs must be heard by somebody.
    pub(in crate::app) fn for_run(owner: &str, active: &mut ActiveRun, turn: AgentTurn) -> Self {
        let choice = active.model_choice.clone();
        let agent_id = active
            .agents
            .ensure_primary(owner, choice, &now_rfc3339())
            .id
            .clone();
        Self::for_run_agent(owner, &agent_id, active, turn)
    }

    /// The same, for a caller that knows which of the run's agents it means:
    /// `run.request_changes` addresses the agent whose conversation the
    /// reviewer was reading, and — when that is the first one — swaps the run's
    /// roster for its Issue's so the comments land on the conversation the
    /// Issue renders. Either way the turn must reach the run's OWN agent.
    pub(in crate::app) fn for_run_agent(
        owner: &str,
        agent_id: &str,
        active: &ActiveRun,
        turn: AgentTurn,
    ) -> Self {
        let agent = active
            .agents
            .by_id(agent_id)
            .expect("a run turn is addressed to one of its agents");
        PendingAgentTurn {
            operation_id: None,
            root: AppState::canonical_root(&active.worktree.path),
            owner: owner.to_string(),
            agent_id: agent_id.to_string(),
            conversation_id: agent.conversation_id().to_string(),
            model_choice: agent.choice.clone(),
            choice_revision: agent.choice_revision,
            interrupt: false,
            say: Some(TurnText {
                cold: turn.cold,
                warm: turn.warm,
            }),
            phase: turn.phase,
            wants_catch_up: true,
            survives_refusal: false,
        }
    }

    /// Address a plan's turn to the primary checkout its planning agent runs
    /// in. `None` once the workspace is gone (approve/abandon drop it): a plan
    /// with no workspace has no agent, and every plan surface renders the empty
    /// state rather than a tab that cannot exist.
    pub(in crate::app) fn for_plan(
        owner: &str,
        active: &ActivePlan,
        turn: AgentTurn,
    ) -> Option<Self> {
        let workspace = active.workspace.as_ref()?;
        let agent_id = active.agents.sole().id.clone();
        Some(PendingAgentTurn {
            operation_id: None,
            root: AppState::canonical_root(&workspace.checkout),
            owner: owner.to_string(),
            conversation_id: active.agents.sole().conversation_id().to_string(),
            model_choice: active.agents.sole().choice.clone(),
            choice_revision: active.agents.sole().choice_revision,
            interrupt: false,
            agent_id,
            say: Some(TurnText {
                cold: turn.cold,
                warm: turn.warm,
            }),
            phase: turn.phase,
            wants_catch_up: true,
            survives_refusal: false,
        })
    }

    /// Address a recovery to the run's primary agent. `None` on an agentless
    /// run: a recovery only exists for an entity that has run, so this is a
    /// refusal rather than a case — and refusing beats minting an agent to
    /// hand a recovery nobody asked for.
    pub(in crate::app) fn for_recovery(
        owner: &str,
        active: &ActiveRun,
        project_root: &std::path::Path,
        prompt: String,
    ) -> Option<Self> {
        // A recovery may replace a dead process or take over a warm
        // implementation tab. In either case it is a distinct Issue agent and
        // must re-establish the durable conversation protocol before touching
        // refs. Wrapping both variants also gives a warm recovery the unread
        // pull instruction instead of assuming an earlier phase primed it.
        //
        // Only the cold half is closed with the catch-up packet at delivery: a
        // warm recovery is a live process that lived this conversation, and
        // the protocol block it keeps already tells it to read what it missed.
        let primed = crate::orchestrator::conversation_prompt(&prompt);
        let agent_id = active.agents.primary()?.id.clone();
        Some(PendingAgentTurn {
            operation_id: None,
            root: AppState::canonical_root(project_root),
            owner: owner.to_string(),
            conversation_id: active
                .agents
                .primary()
                .expect("the recovery agent was just resolved")
                .conversation_id()
                .to_string(),
            model_choice: active
                .agents
                .primary()
                .expect("the recovery agent was just resolved")
                .choice
                .clone(),
            choice_revision: active
                .agents
                .primary()
                .expect("the recovery agent was just resolved")
                .choice_revision,
            interrupt: false,
            agent_id,
            say: Some(TurnText {
                cold: primed.clone(),
                warm: primed,
            }),
            phase: "recover",
            wants_catch_up: true,
            survives_refusal: true,
        })
    }
}

/// The two halves of one turn's text: which one travels is decided by whether
/// the tab had to be spawned to hear it.
///
/// `cold` carries the full run context, because an agent that was just started
/// has none to read the words into; `warm` is the bare instruction, because a
/// live agent is already in the conversation and everything said to it is
/// already durable on the thread for `read_unread_messages` to pull.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::app) struct TurnText {
    pub(in crate::app) cold: String,
    pub(in crate::app) warm: String,
}

/// The live implementation an Issue's conversation actually speaks to: the
/// Issue owns the words, its implementation owns the checkout and the PTY they
/// have to reach. Read off the run before the Issue is taken out of the map, so
/// the agent can be woken — or brought back — without borrowing it again.
pub(in crate::app) struct ImplementationTarget {
    pub(in crate::app) run_id: String,
    pub(in crate::app) worktree_path: std::path::PathBuf,
    pub(in crate::app) agent_id: String,
    pub(in crate::app) model_choice: ModelChoice,
    pub(in crate::app) choice_revision: u64,
}

pub(in crate::app) enum DeliveryOutcome {
    Delivered(Option<(String, Spawned)>),
    /// The exact destination still exists, but changing its frozen model
    /// requires a restart and the current turn has not reached a safe boundary.
    /// No provider call has happened; the durable intent is safe to queue.
    Deferred,
}

pub(in crate::app) enum DeliveryPreflight {
    Proceed { force_fresh: bool },
    Deferred,
    Declined,
}

/// The turns one lock acquisition took off the queue, on their way to their
/// agents.
///
/// A frame answers the moment its own state change is durable, so the queue is
/// taken here and delivered somewhere else. The owners are marked in flight
/// under the SAME acquisition that empties the queue, so there is no instant in
/// which a queued turn is invisible to the idle sweep and its entity looks
/// agentless.
/// The batch OWES those marks back. Each turn settles its own as it lands, and
/// [`Drop`] settles whatever is left, because an owner still marked in flight is
/// spared by the idle sweep forever — a run left Working with no agent and
/// nothing in the daemon able to demote it.
///
/// A mark travels with the turn it was taken for, so the turn that landed is
/// the only one whose mark can be given back. Found by owner instead, a batch
/// carrying two turns for one owner on two agents could settle the OTHER
/// agent's mark and leave its undelivered turn reading as absent.
pub(in crate::app) struct PendingTurns {
    pub(in crate::app) turns: std::collections::VecDeque<(PendingAgentTurn, TurnMark)>,
    pub(in crate::app) state: SettlingHandle,
    pub(in crate::app) clock: Arc<FrameClock>,
}

impl PendingTurns {
    pub(in crate::app) fn is_empty(&self) -> bool {
        self.turns.is_empty()
    }
    pub(in crate::app) fn next_turn(&mut self) -> Option<(PendingAgentTurn, TurnMark)> {
        self.turns.pop_front()
    }
}

impl Drop for PendingTurns {
    fn drop(&mut self) {
        let undelivered = std::mem::take(&mut self.turns);
        if undelivered.is_empty() {
            return;
        }
        self.state.settle(|app| {
            for (_, mark) in undelivered {
                mark.settle(app);
            }
        });
    }
}

pub(in crate::app) struct TurnMark {
    ticket: Option<super::queue::DeliveryTicket>,
    state: SettlingHandle,
}

impl TurnMark {
    fn new(ticket: super::queue::DeliveryTicket, state: SettlingHandle) -> Self {
        Self {
            ticket: Some(ticket),
            state,
        }
    }
    pub(in crate::app) fn settle(mut self, app: &mut AppState) {
        if let Some(ticket) = self.ticket.take() {
            app.delivery_queue.settle(ticket);
        }
    }
}

impl Drop for TurnMark {
    fn drop(&mut self) {
        let Some(ticket) = self.ticket.take() else {
            return;
        };
        self.state
            .settle(move |app| app.delivery_queue.settle(ticket));
    }
}

/// What a delivery reports when the tab it just ensured is already gone.
pub(in crate::app) const TAB_CLOSED_UNDER_A_TURN: &str =
    "the agent tab closed before its turn could be delivered";

/// What a start says on its agent when the entity's session is over and no
/// harness is opened for it — the third answer to a start, beside "live" and
/// a spawn that failed.
pub(in crate::app) const AGENT_START_DECLINED_SESSION_OVER: &str =
    "no session to open: this entity's session is over";

/// Why a screen the spawn was supposed to fill is closed instead.
///
/// Both screens [`ensure_agent_tab`] may be holding — the retained grid of the
/// session being replaced, and the one clients that mounted the Agent tab early
/// are waiting on — exist to be carried onto the new session's screen. A
/// session with no terminal has none, so there is nothing to carry them to and
/// the carry would drop their clients silently: attached to a grid nothing will
/// ever paint, waiting on a basement that is never coming.
///
/// So they are told, the way [`AppState::retire_agent`] and the orphan reaper
/// tell one. The rail reads `has_terminal: false` off the digest by then and
/// stops offering the terminal; this is what closes the door for a client that
/// was already through it.
pub(in crate::app) const NO_TERMINAL_LEFT: &str = "no_terminal";

/// Why a screen a spawn was holding is closed when that spawn never opened.
///
/// The reservation takes the dead session's tab out of the registry and keeps
/// its grid, telling the clients on it NOTHING, because they are about to be
/// handed to the session replacing it. A spawn that fails has nobody to hand
/// them to, and the grid it is holding is in no registry for a reaper or a
/// close to reach: they are told here or they are told never.
pub(in crate::app) const SPAWN_NEVER_OPENED: &str = "spawn_failed";

impl AppState {
    pub(in crate::app) fn take_pending_turns(&mut self) -> PendingTurns {
        let pending_rows = &self.pending_rows;
        let mut ready = self
            .delivery_queue
            .take_ready(|turn| pending_rows.iter().any(|row| row.entity_id == turn.owner));
        for turn in &mut ready {
            self.forget_agent_start_error(&turn.owner, &turn.agent_id);
            if !turn.wants_catch_up {
                continue;
            }
            if let Some(say) = turn.say.as_mut() {
                say.cold = self.cold_prompt_with_catch_up(&turn.owner, &turn.agent_id, &say.cold);
            }
        }
        let state = self.settling_handle();
        let mut turns = std::collections::VecDeque::with_capacity(ready.len());
        for turn in ready {
            let ticket = self.delivery_queue.start(&turn);
            let mark = TurnMark::new(ticket, state.clone());
            turns.push_back((turn, mark));
        }
        PendingTurns {
            turns,
            state,
            clock: Arc::clone(&self.frame_clock),
        }
    }
}
