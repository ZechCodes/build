#[cfg(test)]
use crate::app::NEW_THREAD_MESSAGES_PROMPT;
use crate::app::{
    spawn_tab_pumps, AgentSpawnRequest, AppState, McpTokenLease, SpawnAvailability,
    SpawnClaimToken, Tab, TabKey, TabRole, NO_TERMINAL_LEFT, SPAWN_NEVER_OPENED,
};
use crate::delivery::{AgentSpawnPlan, ReadyToSpawn};
#[cfg(test)]
use crate::harness::Turn;
use crate::harness::{
    SessionIdentitySource, SessionOpenRequest, SessionOutput, TerminalOpenOptions,
};
use crate::models::ModelChoice;
use crate::orchestrator::{ActivePlan, PreparedAgentLaunch};
use crate::screen::{ScreenHandle, TerminalHandle};
use crate::store::now_rfc3339;
use crate::thread::{SessionInstance, SessionStart};
use crate::timing::FrameTimer;
#[cfg(test)]
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Whether [`ensure_agent_tab`] found the tab or created it — the ONE input to
/// the cold/warm decision. Coldness is never re-derived from a transcript
/// probe: a transcript can exist while the process is dead, and a context-free
/// nudge into a resumed session whose structured state has moved is exactly the
/// failure this replaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum Spawned {
    /// The tab already existed and its process is live: the agent is mid
    /// conversation and the thread messages are already durable.
    Warm,
    /// The tab was just created (or its dead process was replaced): the agent
    /// has no context to read messages into.
    Fresh,
}

/// What the lock-held half of a spawn decided.
pub(in crate::app) enum SpawnDecision {
    /// A live tab of this owner's — found on arrival, or waited out.
    Live(String),
    /// The entity's session is over, so there is no agent to open.
    NoSession,
    /// Nobody else is opening this tab, so this caller is. Boxed because a
    /// whole spawn plan dwarfs a wire id, and every decision would pay for it.
    Reserved(Box<ReservedSpawn>),
}

/// What the lock-held half of a spawn hands to the lock-free half.
pub(in crate::app) struct ReservedSpawn {
    pub(in crate::app) plan: AgentSpawnPlan,
    pub(in crate::app) role: TabRole,
    pub(in crate::app) holding: SpawnHolding,
}

/// What a reservation is holding on the registry's behalf until the tab opens.
///
/// Three things the registry gave up when the reservation was taken, and all
/// three go back together if the spawn never opens.
pub(in crate::app) struct SpawnHolding {
    pub(in crate::app) claim: SpawnClaim,
    /// The grid of the dead session this spawn replaces, kept for the session
    /// about to paint it.
    pub(in crate::app) carried: Option<ScreenHandle>,
    /// The exact MCP capability installed for this pending child.
    token_lease: McpTokenLease,
}

#[cfg(test)]
impl SpawnHolding {
    pub(in crate::app) fn test_session_token(&self) -> &str {
        self.token_lease.test_token()
    }
}

impl SpawnHolding {
    /// Give everything back, and hand the caller the reason the spawn never
    /// opened.
    ///
    /// The reservation took the dead session's tab out of the registry and kept
    /// its grid, telling the clients on it NOTHING, because they were about to
    /// be handed to the session replacing it. There is no such session now, and
    /// the grid is in no registry for a reaper or a close to reach: they are
    /// told here or they are told never.
    pub(in crate::app) fn abandon(
        self,
        state: &Arc<Mutex<AppState>>,
        error: String,
        timer: &FrameTimer,
    ) -> String {
        if let Some(screen) = &self.carried {
            screen.close(SPAWN_NEVER_OPENED);
        }
        let mut s = timer.lock(state);
        s.session_registry
            .revoke_mcp_token_if_current(&self.token_lease);
        self.claim.settle(&mut s);
        error
    }
}

/// The child a spawn opened, and the one thing publishing it has to write down.
pub(in crate::app) struct OpenedSession {
    pub(in crate::app) tab: Tab,
    pub(in crate::app) output: SessionOutput,
    /// The provider no longer holds the conversation name this agent's record
    /// carries, so the record has to forget it.
    pub(in crate::app) recorded_name_is_gone: bool,
    pub(in crate::app) claim: SpawnClaim,
}

/// The daemon itself, held the way a background job has to hold it.
///
/// Weakly, because a job that outlives the daemon has nothing to give back to,
/// and through a poisoned mutex deliberately, because a job that ended by
/// panicking still has to settle and a destructor that panics during an unwind
/// aborts the process. Every background job that took something out of the
/// registry before it left gives it back through one of these.
#[derive(Clone)]
pub(in crate::app) struct SettlingHandle(Option<std::sync::Weak<Mutex<AppState>>>);

impl SettlingHandle {
    /// Run `settle` under the app mutex, or not at all if the daemon is gone.
    pub(in crate::app) fn settle(&self, settle: impl FnOnce(&mut AppState)) {
        let Some(state) = self.0.as_ref().and_then(std::sync::Weak::upgrade) else {
            return;
        };
        settle(
            &mut state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
    }
}

/// One worktree's agent spawn, reserved.
///
/// Held from the acquisition that found no live tab to the acquisition that
/// publishes the new one, so two callers of one tab produce one harness — two
/// agents in one worktree would both report `done` for the same owner, and the
/// second report is an illegal transition that lands on the thread as a bogus
/// failure. Every way out of a spawn releases it: [`SpawnClaim::settle`] under
/// a lock the caller already holds, and [`Drop`] on any path that never got
/// there, a panic included.
pub(in crate::app) struct SpawnClaim {
    token: Option<SpawnClaimToken>,
    state: SettlingHandle,
}

impl SpawnClaim {
    pub(in crate::app) fn take(s: &mut AppState, key: &TabKey) -> SpawnClaim {
        SpawnClaim {
            token: Some(s.session_registry.take_spawn_claim(key.clone())),
            state: s.settling_handle(),
        }
    }

    pub(in crate::app) fn settle(mut self, s: &mut AppState) {
        let token = self
            .token
            .take()
            .expect("an unsettled spawn owns its claim");
        s.session_registry.settle_spawn_claim(token);
    }
}

impl Drop for SpawnClaim {
    fn drop(&mut self) {
        let Some(token) = self.token.take() else {
            return;
        };
        self.state.settle(move |s| {
            s.session_registry.settle_spawn_claim(token);
        });
    }
}

/// How long a caller that lost the spawn race waits for the winner's tab before
/// giving up. Comfortably past a harness's own readiness grace, because the
/// winner holds the reservation across it.
pub(in crate::app) const AGENT_SPAWN_WAIT: Duration = Duration::from_secs(30);

/// Find-or-create the one agent tab rooted at `root`.
///
/// Three phases, one call each. **Reserve** decides under the lock: hand back a
/// live tab, wait out the spawn somebody else is already making, or take the
/// reservation. **Open** does the disk work and starts the child with the lock
/// released. **Publish** puts the tab in the registry.
///
/// Idempotent per root: the find half and the in-flight reservation are taken
/// under the SAME lock acquisition, so two concurrent callers produce one
/// harness — two agents in one worktree would both report `done` for the same
/// owner, and the second report is an illegal transition that lands on the
/// thread as a bogus failure. A tab whose process has died is replaced (a dead
/// agent is not an agent), and that replacement reports `Fresh` while carrying
/// the retained screen — and its monotonic cursor — forward.
///
/// The create half needs an owner for the MCP `--task` argv, so it requires a
/// bound plan/run: `owner` resolves the project whose orchestrator builds the
/// spec (the MCP socket lives inside that closure and is unreachable from here).
pub(in crate::app) fn ensure_agent_tab(
    state: &Arc<Mutex<AppState>>,
    root: &std::path::Path,
    request: AgentSpawnRequest<'_>,
    timer: &FrameTimer,
) -> Result<Option<(String, Spawned)>, String> {
    let key = TabKey::agent(&AppState::canonical_root(root), request.agent_id);
    let reserved = match claim_agent_spawn(state, &key, &request, timer)? {
        SpawnDecision::Live(wire_id) => return Ok(Some((wire_id, Spawned::Warm))),
        SpawnDecision::NoSession => return Ok(None),
        SpawnDecision::Reserved(reserved) => *reserved,
    };
    let Some(opened) = open_agent_session(state, reserved, &key, request.conversation_id, timer)?
    else {
        return Ok(None);
    };
    Ok(publish_agent_tab(
        state,
        &key,
        opened,
        request.conversation_id,
        request.model_choice,
        request.phase,
        timer,
    )
    .map(|wire_id| (wire_id, Spawned::Fresh)))
}

/// Decide, under the lock, what this caller is to do about the tab.
///
/// The wait gives the mutex back for its whole duration and wakes on the
/// winner's claim being released, so a caller that lost the race costs the
/// daemon nothing while it waits.
pub(in crate::app) fn claim_agent_spawn(
    state: &Arc<Mutex<AppState>>,
    key: &TabKey,
    request: &AgentSpawnRequest<'_>,
    timer: &FrameTimer,
) -> Result<SpawnDecision, String> {
    let deadline = std::time::Instant::now() + AGENT_SPAWN_WAIT;
    let mut s = timer.lock(state);
    loop {
        if !s.agent_target_exists(
            request.owner,
            request.agent_id,
            request.conversation_id,
            &key.root,
        ) {
            return Ok(SpawnDecision::NoSession);
        }
        match s.session_registry.spawn_availability(
            key,
            request.owner,
            request.agent_id,
            request.force_fresh,
        ) {
            SpawnAvailability::Live(wire_id) => return Ok(SpawnDecision::Live(wire_id)),
            SpawnAvailability::Available => {
                return reserve_agent_spawn(&mut s, key, request)
                    .map(|reserved| SpawnDecision::Reserved(Box::new(reserved)));
            }
            SpawnAvailability::Claimed => {}
        }
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return Err(format!(
                "timed out waiting for the agent starting in {}",
                key.root.display()
            ));
        }
        let finished = s.session_registry.spawn_finished();
        s = s.wait_until(&finished, left, |state| {
            !state.session_registry.claim_is_held(key)
        });
    }
}

/// Take the spawn reservation and read everything the disk work will need.
///
/// Every field of the plan is owned — the project's orchestrator is cloned, the
/// probes are `Arc`s — so nothing it does afterwards can reach back into the
/// registry this read it out of.
///
/// Every read that can fail runs FIRST, with the registry untouched. Retiring
/// the dead tab and registering the MCP token are the reservation giving
/// things up on the registry's behalf, and [`SpawnHolding::abandon`] is the one
/// primitive that gives them back — a failure between the take and the holding
/// would bypass it, leaving browsers on a grid no registry can reach and a
/// token no child holds. So the only failure arm here fails before anything is
/// taken.
pub(in crate::app) fn reserve_agent_spawn(
    s: &mut AppState,
    key: &TabKey,
    request: &AgentSpawnRequest<'_>,
) -> Result<ReservedSpawn, String> {
    let owner = request.owner;
    let agent_id = request.agent_id;
    let conversation_id = request.conversation_id;
    let model_choice = request.model_choice;
    let force_fresh = request.force_fresh;
    if !s.agent_target_exists(owner, agent_id, conversation_id, &key.root) {
        return Err(format!("agent {agent_id} is no longer attached to {owner}"));
    }
    // A router session belongs to no project — deciding which one
    // the capture belongs to is its job. Any project's
    // orchestrator builds the same harness spec for it, since the
    // spec is made from the cwd and the owner id alone.
    let project_id = match s.project_of(owner) {
        Ok(project_id) => project_id,
        Err(unknown) if crate::router::is_router_agent(agent_id) => {
            s.default_project().map_err(|_| unknown)?
        }
        Err(unknown) => return Err(unknown),
    };
    let project = s.orch_for(&project_id)?.clone();
    let replaced = s
        .session_registry
        .agent_snapshot(key)
        .and_then(|tab| tab.instance);
    let carried = s
        .retire_tab_keeping_screen(key)
        .and_then(|(_reaping, screen)| screen);
    if let Some(instance) = replaced {
        s.record_agent_session_end(&instance.entity_id, &instance.agent_id, &instance);
    }
    // A checkout outlives the entity that owned it — a planning
    // worktree is torn down and a run cuts a new one at the same
    // path, an adopted worktree is released and re-adopted. Agents
    // of the entity that USED to own this directory are stale: they
    // would keep working in it and report `done` for an owner that
    // no longer holds it. Several agents of the CURRENT owner are
    // exactly what a branch is allowed to have, so only the others
    // go.
    let stale: Vec<TabKey> = s
        .session_registry
        .agent_tabs_at(&key.root)
        .into_iter()
        .filter(|tab| {
            tab.owner != owner
                && tab.instance.as_ref().is_none_or(|instance| {
                    !s.agent_target_exists(
                        &tab.owner,
                        &tab.agent_id,
                        &instance.conversation_id,
                        &tab.key.root,
                    )
                })
        })
        .map(|tab| tab.key)
        .collect();
    for other in stale {
        let instance = s
            .session_registry
            .agent_snapshot(&other)
            .and_then(|tab| tab.instance);
        s.retire_tab(&other, "closed");
        if let Some(instance) = instance {
            s.record_agent_session_end(&instance.entity_id, &instance.agent_id, &instance);
        }
    }
    let session_token = uuid::Uuid::new_v4().to_string();
    // Before the child exists, because the child dials the done socket as soon
    // as it is up and an unregistered token is an unauthorized report.
    let session_token_lease = s
        .session_registry
        .install_mcp_token(agent_id.to_string(), session_token.clone());
    let recorded_resume_id = (!force_fresh)
        .then(|| s.resumable_session_id(owner, agent_id, &key.root, model_choice.provider))
        .flatten();
    if (force_fresh || recorded_resume_id.is_none())
        && s.recorded_resume_id(owner, agent_id).is_some()
    {
        s.record_agent_resume_id(owner, agent_id, None);
    }
    Ok(ReservedSpawn {
        plan: AgentSpawnPlan {
            project,
            root: key.root.clone(),
            agent_id: agent_id.to_string(),
            model_choice: model_choice.clone(),
            recorded_resume_id,
            probes: s.session_probes(),
            session_token: session_token.clone(),
        },
        role: TabRole::Agent {
            owner: owner.to_string(),
            agent_id: agent_id.to_string(),
            provider: model_choice.provider,
        },
        holding: SpawnHolding {
            claim: SpawnClaim::take(s, key),
            carried,
            token_lease: session_token_lease,
        },
    })
}

/// Open the child, with the app mutex RELEASED.
///
/// The three transcript reads, the `.build/` scaffold and the harness spawn
/// itself: between them they walk a tree the daemon does not own and wait on a
/// harness's readiness, and every terminal pump needs the app mutex while they
/// do. Either step failing gives the whole reservation back before it answers.
/// Everything a provider needs to open the agent `prepared` describes.
///
/// A terminal names its conversation from the launch contract when the spec
/// fixes it, and otherwise from the provider's pre-spawn transcript watcher;
/// a protocol carrier ignores the terminal mechanics and announces its own id.
pub(in crate::app) fn agent_open_request(
    prepared: PreparedAgentLaunch,
    root: std::path::PathBuf,
    model_choice: &ModelChoice,
    resume_session_id: Option<String>,
    _locator: Option<Box<dyn crate::harness::SessionLocator>>,
) -> SessionOpenRequest {
    let identity = match &prepared.spec.known_session_id {
        Some(known) => Some(SessionIdentitySource::Known(known.clone())),
        None => resume_session_id
            .as_ref()
            .map(|verified| SessionIdentitySource::Known(verified.clone())),
    };
    SessionOpenRequest {
        spec: prepared.spec,
        root,
        choice: model_choice.clone(),
        terminal: TerminalOpenOptions {
            size: prepared.pty_size,
            turn_ready_grace: Some(crate::orchestrator::HARNESS_READY_GRACE),
            identity,
        },
        resume_session_id,
    }
}

/// `Ok(None)`: the owner's session ended while the spawn was reserved (a merge
/// pruned the checkout, an issue was approved). The reservation is released
/// and nothing is written to disk, so the pruned directory is not resurrected
/// by the scaffold a spawn would otherwise lay down.
pub(in crate::app) fn open_agent_session(
    state: &Arc<Mutex<AppState>>,
    reserved: ReservedSpawn,
    key: &TabKey,
    conversation_id: &str,
    timer: &FrameTimer,
) -> Result<Option<OpenedSession>, String> {
    let ReservedSpawn {
        plan,
        role,
        holding,
    } = reserved;
    let session_is_over = match &role {
        TabRole::Agent {
            owner, agent_id, ..
        } => !timer
            .lock(state)
            .agent_target_exists(owner, agent_id, conversation_id, &key.root),
        TabRole::Shell => false,
    };
    if session_is_over {
        holding.abandon(state, String::new(), timer);
        return Ok(None);
    }
    let choice = plan.model_choice.clone();
    let opened = plan.probe_and_scaffold().and_then(|ready| {
        let ReadyToSpawn {
            spec,
            size,
            locator,
            resume_session_id,
            recorded_name_is_gone,
        } = ready;
        let TabRole::Agent {
            owner, agent_id, ..
        } = role
        else {
            unreachable!("an agent reservation always names an agent")
        };
        Tab::spawn_agent(
            owner,
            agent_id,
            agent_open_request(
                PreparedAgentLaunch {
                    spec,
                    pty_size: size,
                },
                key.root.clone(),
                &choice,
                resume_session_id,
                locator,
            ),
        )
        .map(|(tab, output)| (tab, output, recorded_name_is_gone))
    });
    match opened {
        Ok((mut tab, output, recorded_name_is_gone)) => {
            if let Some(screen) = holding.carried {
                tab.adopt_screen(screen);
            }
            Ok(Some(OpenedSession {
                tab,
                output,
                recorded_name_is_gone,
                claim: holding.claim,
            }))
        }
        Err(error) => Err(holding.abandon(state, error, timer)),
    }
}

/// Put the opened tab in the registry and start its pumps.
///
/// `None` means the tab was stranded: the entity lost its session while this
/// harness was starting — an issue approved under its own planning agent. The
/// insert is the instant the agent becomes addressable, so it is the instant
/// the gate that closed has to reach it, and no earlier check is atomic with
/// it.
pub(in crate::app) fn publish_agent_tab(
    state: &Arc<Mutex<AppState>>,
    key: &TabKey,
    opened: OpenedSession,
    conversation_id: &str,
    model_choice: &ModelChoice,
    phase: &str,
    timer: &FrameTimer,
) -> Option<String> {
    let OpenedSession {
        tab,
        output,
        recorded_name_is_gone,
        claim,
    } = opened;
    let (owner, agent_id) = tab
        .role
        .agent()
        .map(|(owner, agent_id)| (owner.to_string(), agent_id.to_string()))
        .expect("an agent spawn opens an agent tab");
    let wire_id = tab.wire_id();
    let mut pumps = None;
    let inherited;
    let stranded;
    {
        let mut s = timer.lock(state);
        // The probe read the recorded name and the provider no longer holds it.
        // Forgetting it is a state write, so it happens here rather than in the
        // probe that found out.
        if recorded_name_is_gone {
            s.record_agent_resume_id(&owner, &agent_id, None);
        }
        inherited = inherit_waiting_clients(&mut s, key, &tab);
        let running = tab
            .session
            .active_model()
            .or_else(|| model_choice.model.clone());
        s.session_registry.insert_opened(key.clone(), tab);
        claim.settle(&mut s);
        s.record_agent_active_model(&owner, &agent_id, running);
        stranded = !s.agent_target_exists(&owner, &agent_id, conversation_id, &key.root);
        if stranded {
            s.retire_tab(key, "closed");
        } else {
            let instance =
                s.record_agent_session_start(&owner, &agent_id, &key.root, model_choice, phase);
            pumps = Some(
                s.session_registry
                    .set_instance_and_take_pumps(key, instance, output),
            );
        }
    }
    if stranded {
        return None;
    }
    if let Some(inherited) = inherited {
        inherited.fit_child_to_screen();
    }
    spawn_tab_pumps(
        state,
        key.clone(),
        pumps.expect("a non-stranded tab starts its pumps"),
    );
    Some(wire_id)
}

/// Move the clients that were waiting for `tab`'s agent onto the screen it will
/// paint, and hand back the child's half of the move.
///
/// Clients that mounted the Agent tab before this worktree had one are attached
/// to a screen with no PTY. They are carried — with the viewport they render
/// at, the same rule an attach to a live tab follows — onto the real screen.
/// The carry is what makes the waiting screen point at this one, so a client
/// attaching during the spawn is on one screen or the other and never between
/// them however the two acquisitions fall. The waiting screen's cursor is not
/// carried: it painted nothing, while a retained screen's cursor is the one
/// that must never rewind.
///
/// The screen half is bounded and belongs under the app mutex, beside the
/// insert that publishes the tab. The child half is an ioctl to a process that
/// may not answer, so what comes back is the terminal that inherited them, for
/// the caller to fit to its screen with the lock down.
pub(in crate::app) fn inherit_waiting_clients(
    s: &mut AppState,
    key: &TabKey,
    tab: &Tab,
) -> Option<TerminalHandle> {
    let fallback = TabKey::agent(&key.root, &crate::worktree::external_worktree_id(&key.root));
    s.session_registry
        .inherit_waiting_clients(key, tab, &fallback, NO_TERMINAL_LEFT)
}

/// Tell the worktree's agent, in place, that unread thread messages await.
///
/// [`deliver`]'s warm branch without the cold half: it notifies whatever agent
/// is ALIVE in that worktree, whatever its entity is parked as, and does
/// nothing at all for one that is not. Deciding between the two — and starting
/// the agent that is not running — belongs to
/// [`AppState::tell_the_agent_a_message_is_waiting`], the only caller.
///
/// This was once gated on `building`/`drafting`, from when the agent existed
/// only while working: any other state meant no process to talk to. A worktree's
/// agent now outlives every phase and is sitting in the Agent tab the human is
/// typing into, so gating on entity state meant a message could be typed into a
/// live conversation and silently not arrive. A `done` the run machine does not
/// accept from that state is recorded as out-of-phase and moves nothing, which
/// is a far smaller cost than a conversation that lies about itself.
///
/// No tab, or a tab whose process has ended, swallows the nudge, and a write
/// failure against an exiting harness is logged, never surfaced: the message is
/// durable either way.
///
/// Unlike [`deliver`], this speaks from under the app-wide state lock — it
/// reads the caller's own tab registry — which is why
/// [`AgentSession::send_turn`] must return promptly. A session that blocked
/// there would stall every RPC and every terminal pump behind one nudge.
#[cfg(test)]
pub(in crate::app) fn nudge_live_agent_tab(
    tabs: &HashMap<TabKey, Tab>,
    root: &std::path::Path,
    agent_id: &str,
    entity_id: &str,
    interrupt: bool,
) {
    let Some(tab) = tabs.get(&TabKey::agent(&AppState::canonical_root(root), agent_id)) else {
        return;
    };
    if !tab.session_is_live() {
        return;
    }
    // Stop first, then hand over — the order is the whole point of the flag
    // riding the message rather than arriving as a verb of its own, which would
    // leave a window in which the child starts a fresh turn or the agent calls
    // `done`. Both calls return promptly by contract, which is what lets them
    // speak from under the state lock.
    //
    // A refusal is not a failed post. Where the session cannot stop a turn —
    // a capability lost between the digest the client read and the post it sent
    // — the message is delivered as an ordinary queued turn, which reaches the
    // running turn at its next step boundary anyway. The alternative is an
    // error the human must read for a difference they cannot act on and did not
    // cause.
    if interrupt {
        if let Err(refused) = tab.session.interrupt() {
            eprintln!("thread.post {entity_id}: interrupt refused: {refused}");
        }
    }
    // As a turn, not a raw write with a hardcoded Enter: the nudge is one of
    // Build's turns, so it travels the way every other one does and the
    // session decides what that means. Hardcoding \r submits into a SubmitKey::None
    // harness that never asked for it, leaves the notification unframed — and
    // says nothing at all to a session with no keyboard.
    if let Err(error) = tab
        .session
        .send_turn(&Turn::new(NEW_THREAD_MESSAGES_PROMPT))
    {
        eprintln!("thread.post {entity_id}: agent notify failed: {error}");
    }
}

/// Where an issue's one agent is running right now — its checkout and its
/// agent id — or `None` when the issue has no session. Read BEFORE a verb that
/// ends the session, since ending it is what clears the workspace.
pub(in crate::app) fn issue_session(active: &ActivePlan) -> Option<(std::path::PathBuf, String)> {
    let workspace = active.workspace.as_ref()?;
    Some((workspace.checkout.clone(), active.agents.sole().id.clone()))
}

/// Open a conversation's session lineage for a newly spawned agent process,
/// chaining it off the previous session so the thread still reads as a chain.
pub(in crate::app) fn open_session_lineage(
    thread: &mut crate::thread::Thread,
    entity_id: &str,
    agent_id: &str,
    checkout: &str,
    model_choice: &ModelChoice,
    phase: &str,
) -> SessionInstance {
    let now = now_rfc3339();
    let instance = thread.start_agent_session(SessionStart {
        entity_id,
        agent_id,
        checkout,
        provider: model_choice.provider.label(),
        model: model_choice.model.as_deref(),
        effort: model_choice.effort.as_deref(),
        phase,
        now: &now,
    });
    thread.push_event(
        crate::thread::ThreadEventKind::RunStarted,
        Some(format!("{phase} run started")),
        Some(instance.id.clone()),
        None,
        now_rfc3339(),
    );
    instance
}

impl AppState {
    /// This daemon, held the way a background job has to hold it — see
    /// [`SettlingHandle`].
    pub(in crate::app) fn settling_handle(&self) -> SettlingHandle {
        SettlingHandle(self.self_handle.clone())
    }
}
