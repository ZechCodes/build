//! The registry owns no AppState callback and performs no filesystem,
//! persistence, process wait, or conversation mutation.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Condvar};
use std::time::Instant;

use serde_json::Value;

use crate::harness::{AgentSession, AgentStatus, SessionOutput};
use crate::reaper::Retirement;
use crate::screen::{ScreenHandle, TerminalHandle};
use crate::thread::SessionInstance;

use super::{LifecycleDiagnostic, MintedCallRow, Tab, TabKey, TabPumps, TabRole};
use crate::app::runtime::terminals::{TabAttachment, TabFacts};

/// Compare an MCP token without making the compared byte position depend on
/// either input's length. This is the exact body currently used by AppState.
pub(in crate::app) fn constant_time_token_eq(actual: &str, expected: &str) -> bool {
    let actual = actual.as_bytes();
    let expected = expected.as_bytes();
    let mut difference = actual.len() ^ expected.len();
    let width = actual.len().max(expected.len());
    for index in 0..width {
        difference |= usize::from(
            actual.get(index).copied().unwrap_or(0) ^ expected.get(index).copied().unwrap_or(0),
        );
    }
    difference == 0
}

/// All daemon-local terminal and agent-session registrations.
///
/// The enclosing AppState mutex remains the only lock protecting this value.
/// In particular, `spawn_finished` is paired with that original mutex guard;
/// SessionRegistry must never acquire an independent mutex.
pub(in crate::app) struct SessionRegistry {
    tabs: HashMap<TabKey, Tab>,
    spawn_claims: HashSet<TabKey>,
    spawn_finished: Arc<Condvar>,
    waiting_screens: HashMap<TabKey, ScreenHandle>,
    mcp_tokens: HashMap<String, String>,
    next_terminal: u64,
    /// Where the tabs' byte pumps paint: the daemon's push runtime, or — with
    /// none set, as in the tests — whatever runtime starts them.
    push_runtime: Option<tokio::runtime::Handle>,
}

/// A claimed spawn key. Runtime owns the RAII wrapper that settles this token
/// through a weak AppState handle if a spawn unwinds.
pub(in crate::app) struct SpawnClaimToken {
    key: TabKey,
}

/// The exact token installed for one pending child. Conditional revocation
/// prevents an old cancellation from removing a newer child's token.
#[derive(Clone)]
pub(in crate::app) struct McpTokenLease {
    agent_id: String,
    token: String,
}

#[cfg(test)]
impl McpTokenLease {
    pub(in crate::app) fn test_token(&self) -> &str {
        &self.token
    }
}

/// What the existing spawn loop can observe while holding the app guard.
pub(in crate::app) enum SpawnAvailability {
    Live(String),
    Claimed,
    Available,
}

/// Stable facts used by the adapter's exact stale-owner predicate.
pub(in crate::app) struct AgentTabFact {
    pub key: TabKey,
    pub owner: String,
    pub agent_id: String,
    pub instance: Option<SessionInstance>,
}

pub(in crate::app) struct ShellTabFact {
    pub key: TabKey,
    pub tab_id: String,
    pub created_at: String,
    pub size: Option<(u16, u16)>,
}

/// Owned facts for preflight, capture, digest, and callback adapters. Session
/// behavior is queried after release only where the current caller already does
/// so; application-owned conversation and model lookups remain in the adapter.
pub(in crate::app) struct AgentSessionSnapshot {
    pub key: TabKey,
    pub role: TabRole,
    pub session: Arc<dyn AgentSession>,
    pub instance: Option<SessionInstance>,
    pub live: bool,
}

pub(in crate::app) struct AgentDigestFacts {
    pub live: bool,
    pub working: bool,
    pub has_terminal: bool,
    pub can_interrupt: bool,
    pub surface_session_generation: Option<String>,
    pub surfaces: Option<Value>,
}

pub(in crate::app) enum IdleObservation {
    Active,
    Idle {
        exit_code: Option<i32>,
        epitaph: Option<String>,
    },
}

/// Registry-owned removal result. Conversation lineage is settled by the app
/// adapter only after every selected tab has been removed.
pub(in crate::app) struct RetiredAgent {
    pub wire_id: String,
    pub instance: Option<SessionInstance>,
    pub retirement: Retirement,
}

/// Removal that intentionally keeps the screen and says nothing to its clients.
pub(in crate::app) struct RetainedTab {
    pub retirement: Retirement,
    pub screen: Option<ScreenHandle>,
}

impl SessionRegistry {
    pub(in crate::app) fn new() -> Self {
        Self {
            tabs: HashMap::new(),
            spawn_claims: HashSet::new(),
            spawn_finished: Arc::new(Condvar::new()),
            waiting_screens: HashMap::new(),
            mcp_tokens: HashMap::new(),
            next_terminal: 1,
            push_runtime: None,
        }
    }

    pub(in crate::app) fn set_push_runtime(&mut self, runtime: tokio::runtime::Handle) {
        self.push_runtime = Some(runtime);
    }

    /// A tab's pumps, told where to paint.
    fn pumps_of(&self, tab: &Tab, output: SessionOutput) -> TabPumps {
        TabPumps {
            push_runtime: self.push_runtime.clone(),
            ..tab.pumps(output)
        }
    }

    pub(in crate::app) fn next_terminal_id(&mut self) -> String {
        let id = format!("term-{}", self.next_terminal);
        self.next_terminal += 1;
        id
    }

    pub(in crate::app) fn shell_count(&self) -> usize {
        self.tabs
            .values()
            .filter(|tab| tab.role == TabRole::Shell)
            .count()
    }

    pub(in crate::app) fn contains(&self, key: &TabKey) -> bool {
        self.tabs.contains_key(key)
    }

    pub(in crate::app) fn key_for_wire_id(&self, wire_id: &str) -> Result<TabKey, String> {
        self.tabs
            .iter()
            .find(|(_, tab)| tab.wire_id() == wire_id)
            .map(|(key, _)| key.clone())
            .ok_or_else(|| "unknown term_id".to_string())
    }

    pub(in crate::app) fn agent_is_live(&self, key: &TabKey) -> bool {
        self.tabs.get(key).is_some_and(Tab::session_is_live)
    }

    pub(in crate::app) fn agent_working_roots(&self) -> Vec<(std::path::PathBuf, bool)> {
        self.tabs
            .iter()
            .filter(|(_, tab)| tab.role.agent().is_some())
            .map(|(_, tab)| {
                (
                    tab.root.clone(),
                    tab.live && matches!(tab.session.status(), AgentStatus::Working),
                )
            })
            .collect()
    }

    pub(in crate::app) fn insert_shell(
        &mut self,
        key: TabKey,
        tab: Tab,
        output: SessionOutput,
    ) -> TabPumps {
        let pumps = self.pumps_of(&tab, output);
        self.tabs.insert(key, tab);
        pumps
    }

    /// Current live reuse semantics include owner and agent identity because
    /// TabKey does not contain owner identity.
    pub(in crate::app) fn spawn_availability(
        &self,
        key: &TabKey,
        expected_owner: &str,
        expected_agent: &str,
        force_fresh: bool,
    ) -> SpawnAvailability {
        if let Some(tab) = self.tabs.get(key) {
            let same_target = tab
                .role
                .agent()
                .is_some_and(|(owner, agent)| owner == expected_owner && agent == expected_agent);
            if same_target && tab.session_is_live() && !force_fresh {
                return SpawnAvailability::Live(tab.wire_id());
            }
        }
        if self.spawn_claims.contains(key) {
            SpawnAvailability::Claimed
        } else {
            SpawnAvailability::Available
        }
    }

    /// Infallible under the same uninterrupted AppState guard that observed
    /// `Available`. This preserves the existing branch structure.
    pub(in crate::app) fn take_spawn_claim(&mut self, key: TabKey) -> SpawnClaimToken {
        self.spawn_claims.insert(key.clone());
        SpawnClaimToken { key }
    }

    pub(in crate::app) fn spawn_finished(&self) -> Arc<Condvar> {
        Arc::clone(&self.spawn_finished)
    }

    pub(in crate::app) fn claim_is_held(&self, key: &TabKey) -> bool {
        self.spawn_claims.contains(key)
    }

    /// The adapter must call this before an under-lock RAII wrapper is dropped.
    pub(in crate::app) fn settle_spawn_claim(&mut self, claim: SpawnClaimToken) {
        self.spawn_claims.remove(&claim.key);
        self.spawn_finished.notify_all();
    }

    pub(in crate::app) fn install_mcp_token(
        &mut self,
        agent_id: String,
        token: String,
    ) -> McpTokenLease {
        self.mcp_tokens.insert(agent_id.clone(), token.clone());
        McpTokenLease { agent_id, token }
    }

    pub(in crate::app) fn token_matches(&self, agent_id: &str, supplied: &str) -> bool {
        self.mcp_tokens
            .get(agent_id)
            .is_some_and(|expected| constant_time_token_eq(supplied, expected))
    }

    pub(in crate::app) fn revoke_mcp_token(&mut self, agent_id: &str) {
        self.mcp_tokens.remove(agent_id);
    }

    pub(in crate::app) fn revoke_mcp_token_if_current(&mut self, lease: &McpTokenLease) {
        if self
            .mcp_tokens
            .get(&lease.agent_id)
            .is_some_and(|current| constant_time_token_eq(current, &lease.token))
        {
            self.mcp_tokens.remove(&lease.agent_id);
        }
    }

    pub(in crate::app) fn agent_tabs_at(&self, root: &Path) -> Vec<AgentTabFact> {
        self.tabs
            .iter()
            .filter(|(key, _)| key.root == root)
            .filter_map(|(key, tab)| {
                let (owner, agent_id) = tab.role.agent()?;
                Some(AgentTabFact {
                    key: key.clone(),
                    owner: owner.to_string(),
                    agent_id: agent_id.to_string(),
                    instance: tab.session_instance.clone(),
                })
            })
            .collect()
    }

    /// Owned keys let filesystem adapters decide which roots vanished without
    /// borrowing registry storage across that work.
    pub(in crate::app) fn tab_keys(&self) -> Vec<TabKey> {
        self.tabs.keys().cloned().collect()
    }

    pub(in crate::app) fn session_instance(&self, key: &TabKey) -> Option<SessionInstance> {
        self.tabs.get(key)?.session_instance.clone()
    }

    /// Whether this key holds one of the human's shells — a row `term.list`
    /// carries — rather than a worktree's agent.
    pub(in crate::app) fn tab_is_shell(&self, key: &TabKey) -> bool {
        self.tabs
            .get(key)
            .is_some_and(|tab| tab.role == TabRole::Shell)
    }

    pub(in crate::app) fn shell_tabs_at(&self, root: &Path) -> Vec<ShellTabFact> {
        self.tabs
            .iter()
            .filter(|(key, tab)| key.root == root && tab.role == TabRole::Shell)
            .map(|(key, tab)| ShellTabFact {
                key: key.clone(),
                tab_id: tab.tab_id.clone(),
                created_at: tab.created_at.clone(),
                size: tab.screen.as_ref().map(ScreenHandle::size),
            })
            .collect()
    }

    pub(in crate::app) fn waiting_screen_keys(&self) -> Vec<TabKey> {
        self.waiting_screens.keys().cloned().collect()
    }

    /// Session detach needs every live and waiting screen, but no map identity.
    pub(in crate::app) fn screen_handles(&self) -> Vec<ScreenHandle> {
        self.tabs
            .values()
            .filter_map(|tab| tab.screen.clone())
            .chain(self.waiting_screens.values().cloned())
            .collect()
    }

    pub(in crate::app) fn attachment(&self, key: &TabKey) -> Result<TabAttachment, String> {
        let tab = self.tabs.get(key).ok_or("unknown term_id")?;
        Ok(TabAttachment {
            facts: TabFacts {
                term_id: tab.wire_id(),
                live: tab.live,
                provider: match tab.role {
                    TabRole::Agent { provider, .. } => Some(provider),
                    TabRole::Shell => None,
                },
            },
            terminal: tab.terminal_handle()?,
        })
    }

    /// The exact entry-or-existing operation used by `agent_attach`. Returning
    /// a clone preserves a concurrent spawn's client-carry behavior after the
    /// app guard is released.
    pub(in crate::app) fn waiting_screen_for_attach(
        &mut self,
        key: TabKey,
        term_id: &str,
        cols: u16,
        rows: u16,
    ) -> ScreenHandle {
        self.waiting_screens
            .entry(key)
            .or_insert_with(|| ScreenHandle::new(term_id, cols, rows))
            .clone()
    }

    pub(in crate::app) fn first_agent_id_at(&self, root: &Path) -> Option<String> {
        self.tabs
            .keys()
            .find(|key| key.is_agent() && key.root == root)
            .and_then(|key| key.tab_id.strip_prefix("agent:").map(str::to_string))
    }

    /// Input and resize both acquire the terminal capability before consulting
    /// liveness. The adapter performs the potentially blocking operation after
    /// releasing the app guard.
    pub(in crate::app) fn terminal_access(
        &self,
        key: &TabKey,
    ) -> Result<(bool, TerminalHandle), String> {
        let tab = self.tabs.get(key).ok_or("unknown term_id")?;
        let terminal = tab.terminal_handle()?;
        Ok((tab.session_is_live(), terminal))
    }

    pub(in crate::app) fn terminal_resize_access(
        &self,
        key: &TabKey,
    ) -> Result<(bool, TerminalHandle), String> {
        let tab = self.tabs.get(key).ok_or("unknown term_id")?;
        let live = tab.session_is_live();
        Ok((live, tab.terminal_handle()?))
    }

    pub(in crate::app) fn terminal_screen(&self, key: &TabKey) -> Result<ScreenHandle, String> {
        let tab = self.tabs.get(key).ok_or("unknown term_id")?;
        Ok(tab.terminal_handle()?.screen().clone())
    }

    pub(in crate::app) fn agent_snapshot(&self, key: &TabKey) -> Option<AgentSessionSnapshot> {
        let tab = self.tabs.get(key)?;
        tab.role.agent()?;
        Some(AgentSessionSnapshot {
            key: key.clone(),
            role: tab.role.clone(),
            session: Arc::clone(&tab.session),
            instance: tab.session_instance.clone(),
            live: tab.live,
        })
    }

    pub(in crate::app) fn log_if_agent_current(
        &self,
        key: &TabKey,
        session: &Arc<dyn AgentSession>,
        instance: &SessionInstance,
        diagnostic: LifecycleDiagnostic<'_>,
    ) -> bool {
        let Some(tab) = self.tabs.get(key) else {
            return false;
        };
        if !Arc::ptr_eq(&tab.session, session)
            || tab.session_instance.as_ref() != Some(instance)
            || tab.role.agent() != Some((instance.entity_id.as_str(), instance.agent_id.as_str()))
        {
            return false;
        }
        tab.log_lifecycle(diagnostic);
        true
    }

    pub(in crate::app) fn agent_digest_facts(
        &self,
        key: &TabKey,
        include_surfaces: bool,
    ) -> Option<AgentDigestFacts> {
        let tab = self.tabs.get(key)?;
        tab.role.agent()?;
        let live = tab.session_is_live();
        let working = tab.live && matches!(tab.session.status(), AgentStatus::Working);
        let surfaces = include_surfaces
            .then(|| {
                tab.session.surfaces().map(|snapshot| {
                    snapshot.wire_value(&|call_id| {
                        tab.call_sequences.get(call_id).map(|row| row.sequence)
                    })
                })
            })
            .flatten();
        Some(AgentDigestFacts {
            live,
            working,
            has_terminal: tab.session.terminal().is_some(),
            can_interrupt: tab.session.can_interrupt(),
            surface_session_generation: tab
                .session_instance
                .as_ref()
                .map(|instance| instance.id.clone()),
            surfaces,
        })
    }

    /// Evaluate the current idle closure's tab-local half. Missing-tab and
    /// undelivered-turn handling remain with the application adapter.
    pub(in crate::app) fn idle_observation(
        &self,
        key: &TabKey,
        quiet_threshold: std::time::Duration,
    ) -> Option<IdleObservation> {
        let tab = self.tabs.get(key)?;
        let status = tab.session.status();
        if let AgentStatus::Ended { code } = status {
            return Some(IdleObservation::Idle {
                exit_code: Some(code.unwrap_or(-1)),
                epitaph: tab
                    .screen
                    .as_ref()
                    .and_then(ScreenHandle::epitaph)
                    .or_else(|| tab.session.epitaph()),
            });
        }
        if matches!(status, AgentStatus::Working) {
            return Some(IdleObservation::Active);
        }
        let heard_from_recently = tab.session.quiet_for() < quiet_threshold;
        let spoken_to_recently = tab
            .last_delivered_at
            .is_some_and(|at| at.elapsed() < quiet_threshold);
        Some(if heard_from_recently || spoken_to_recently {
            IdleObservation::Active
        } else {
            IdleObservation::Idle {
                exit_code: None,
                epitaph: None,
            }
        })
    }

    pub(in crate::app) fn live_agent_snapshots(&self) -> Vec<AgentSessionSnapshot> {
        self.tabs
            .iter()
            .filter(|(_, tab)| tab.live && tab.role.agent().is_some())
            .map(|(key, tab)| AgentSessionSnapshot {
                key: key.clone(),
                role: tab.role.clone(),
                session: Arc::clone(&tab.session),
                instance: tab.session_instance.clone(),
                live: tab.live,
            })
            .collect()
    }

    /// Delivery writeback after the off-lock send. The same three-part fence
    /// used by agent callbacks guards the mutation.
    pub(in crate::app) fn mark_delivered_if_current(
        &mut self,
        key: &TabKey,
        session: &Arc<dyn AgentSession>,
        instance: &SessionInstance,
        at: Instant,
    ) -> bool {
        let Some(tab) = self.tabs.get_mut(key) else {
            return false;
        };
        if !Arc::ptr_eq(&tab.session, session)
            || tab.session_instance.as_ref() != Some(instance)
            || tab.role.agent() != Some((instance.entity_id.as_str(), instance.agent_id.as_str()))
        {
            return false;
        }
        tab.last_delivered_at = Some(at);
        true
    }

    pub(in crate::app) fn parent_call_sequence(&self, key: &TabKey, call_id: &str) -> Option<u64> {
        Some(self.tabs.get(key)?.call_sequences.get(call_id)?.sequence)
    }

    pub(in crate::app) fn insert_call_sequence(
        &mut self,
        key: &TabKey,
        call_id: String,
        sequence: u64,
    ) -> bool {
        let Some(tab) = self.tabs.get_mut(key) else {
            return false;
        };
        tab.call_sequences.insert(
            call_id,
            MintedCallRow {
                sequence,
                answered: false,
            },
        );
        true
    }

    pub(in crate::app) fn mark_call_answered(
        &mut self,
        key: &TabKey,
        call_id: &str,
    ) -> Option<u64> {
        let row = self.tabs.get_mut(key)?.call_sequences.get_mut(call_id)?;
        row.answered = true;
        Some(row.sequence)
    }

    /// Activity EOF's single guarded registry mutation: mark the exact session
    /// dead and drain its open call rows before the adapter records their
    /// answers and lineage in application-owned stores.
    pub(in crate::app) fn end_agent_stream_if_current(
        &mut self,
        key: &TabKey,
        session: &Arc<dyn AgentSession>,
        instance: &SessionInstance,
    ) -> Option<Vec<u64>> {
        let tab = self.tabs.get_mut(key)?;
        if !Arc::ptr_eq(&tab.session, session)
            || tab.session_instance.as_ref() != Some(instance)
            || tab.role.agent() != Some((instance.entity_id.as_str(), instance.agent_id.as_str()))
        {
            return None;
        }
        tab.live = false;
        let mut unanswered: Vec<u64> = std::mem::take(&mut tab.call_sequences)
            .into_values()
            .filter(|row| !row.answered)
            .map(|row| row.sequence)
            .collect();
        unanswered.sort_unstable();
        Some(unanswered)
    }

    pub(in crate::app) fn shell_pump_matches(
        &self,
        key: &TabKey,
        session: &Arc<dyn AgentSession>,
    ) -> bool {
        self.tabs
            .get(key)
            .is_some_and(|tab| Arc::ptr_eq(&tab.session, session))
    }

    pub(in crate::app) fn agent_pump_matches(
        &self,
        key: &TabKey,
        session: &Arc<dyn AgentSession>,
        instance: &SessionInstance,
    ) -> bool {
        self.tabs.get(key).is_some_and(|tab| {
            Arc::ptr_eq(&tab.session, session)
                && tab.session_instance.as_ref() == Some(instance)
                && tab.role.agent()
                    == Some((instance.entity_id.as_str(), instance.agent_id.as_str()))
        })
    }

    pub(in crate::app) fn mark_agent_ended_if_current(
        &mut self,
        key: &TabKey,
        session: &Arc<dyn AgentSession>,
        instance: &SessionInstance,
        diagnostic: LifecycleDiagnostic<'_>,
    ) -> bool {
        let Some(tab) = self.tabs.get_mut(key) else {
            return false;
        };
        if !Arc::ptr_eq(&tab.session, session)
            || tab.session_instance.as_ref() != Some(instance)
            || tab.role.agent() != Some((instance.entity_id.as_str(), instance.agent_id.as_str()))
        {
            return false;
        }
        tab.log_lifecycle(diagnostic);
        tab.live = false;
        true
    }

    /// Publication step 2. It runs under the same app acquisition as all other
    /// publication steps; fitting the returned terminal runs after release.
    pub(in crate::app) fn inherit_waiting_clients(
        &mut self,
        key: &TabKey,
        tab: &Tab,
        fallback_key: &TabKey,
        no_terminal_reason: &str,
    ) -> Option<TerminalHandle> {
        let first_agent_at_root = !self
            .tabs
            .keys()
            .any(|other| other.is_agent() && other.root == key.root);
        let waiting = self.waiting_screens.remove(key).or_else(|| {
            first_agent_at_root
                .then(|| self.waiting_screens.remove(fallback_key))
                .flatten()
        })?;
        match tab.terminal_handle() {
            Ok(terminal) => terminal
                .screen()
                .carry_clients_from(&waiting)
                .then_some(terminal),
            Err(_) => {
                waiting.close(no_terminal_reason);
                None
            }
        }
    }

    /// Publication step 3. Existing insertion is intentionally infallible.
    pub(in crate::app) fn insert_opened(&mut self, key: TabKey, tab: Tab) {
        self.tabs.insert(key, tab);
    }

    /// Publication step 7, after the app adapter records lineage.
    pub(in crate::app) fn set_instance_and_take_pumps(
        &mut self,
        key: &TabKey,
        instance: Option<SessionInstance>,
        output: SessionOutput,
    ) -> TabPumps {
        let tab = self
            .tabs
            .get_mut(key)
            .expect("the published tab remains present during one app acquisition");
        tab.session_instance = instance;
        let tab = &self.tabs[key];
        self.pumps_of(tab, output)
    }

    pub(in crate::app) fn retain_screen_for_replacement(
        &mut self,
        key: &TabKey,
        diagnostic: LifecycleDiagnostic<'_>,
    ) -> Option<RetainedTab> {
        let tab = self.tabs.remove(key)?;
        tab.log_lifecycle(diagnostic);
        Some(RetainedTab {
            retirement: Retirement::begin(tab.session),
            screen: tab.screen,
        })
    }

    /// Log and remove one tab at the same per-tab boundary as the source.
    pub(in crate::app) fn retire_tab(
        &mut self,
        key: &TabKey,
        reason: &str,
        diagnostic: LifecycleDiagnostic<'_>,
    ) -> Option<RetiredAgent> {
        let tab = self.tabs.remove(key)?;
        tab.log_lifecycle(diagnostic);
        let wire_id = tab.wire_id();
        if let Some(screen) = &tab.screen {
            screen.close(reason);
        }
        Some(RetiredAgent {
            wire_id,
            instance: tab.session_instance,
            retirement: Retirement::begin(tab.session),
        })
    }

    /// Called only after the adapter has recorded every killed tab instance and
    /// then performed the source's delayed waiting-key discovery.
    pub(in crate::app) fn remove_waiting_screen(
        &mut self,
        key: &TabKey,
        reason: &str,
    ) -> Option<String> {
        let screen = self.waiting_screens.remove(key)?;
        screen.close(reason);
        Some(key.tab_id.clone())
    }

    #[cfg(test)]
    pub(in crate::app) fn test_insert_tab(&mut self, key: TabKey, tab: Tab) -> Option<Tab> {
        self.tabs.insert(key, tab)
    }

    #[cfg(test)]
    pub(in crate::app) fn test_tab(&self, key: &TabKey) -> Option<&Tab> {
        self.tabs.get(key)
    }

    #[cfg(test)]
    pub(in crate::app) fn test_tab_mut(&mut self, key: &TabKey) -> Option<&mut Tab> {
        self.tabs.get_mut(key)
    }

    #[cfg(test)]
    pub(in crate::app) fn test_remove_tab(&mut self, key: &TabKey) -> Option<Tab> {
        self.tabs.remove(key)
    }

    #[cfg(test)]
    pub(in crate::app) fn test_tabs(&self) -> impl Iterator<Item = (&TabKey, &Tab)> {
        self.tabs.iter()
    }

    #[cfg(test)]
    pub(in crate::app) fn test_token(&self, agent_id: &str) -> Option<&str> {
        self.mcp_tokens.get(agent_id).map(String::as_str)
    }

    #[cfg(test)]
    pub(in crate::app) fn test_tokens(&self) -> impl Iterator<Item = (&str, &str)> {
        self.mcp_tokens
            .iter()
            .map(|(agent, token)| (agent.as_str(), token.as_str()))
    }

    #[cfg(test)]
    pub(in crate::app) fn test_install_token(&mut self, agent_id: String, token: String) {
        self.mcp_tokens.insert(agent_id, token);
    }

    #[cfg(test)]
    pub(in crate::app) fn test_waiting_screens(&self) -> impl Iterator<Item = &ScreenHandle> {
        self.waiting_screens.values()
    }

    #[cfg(test)]
    pub(in crate::app) fn test_remember_waiting_screen(
        &mut self,
        key: TabKey,
        screen: ScreenHandle,
    ) {
        self.waiting_screens.insert(key, screen);
    }

    #[cfg(test)]
    pub(in crate::app) fn test_counts(&self) -> SessionRegistryCounts {
        SessionRegistryCounts {
            tabs: self.tabs.len(),
            claims: self.spawn_claims.len(),
            waiting_screens: self.waiting_screens.len(),
            tokens: self.mcp_tokens.len(),
        }
    }
}

#[cfg(test)]
pub(in crate::app) struct SessionRegistryCounts {
    pub tabs: usize,
    pub claims: usize,
    pub waiting_screens: usize,
    pub tokens: usize,
}
