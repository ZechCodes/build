//! Committed MCP addressing, independent of SQL histories and runtime tabs.
use super::AppState;
use crate::agent::NO_AGENT_YET;
use crate::mcp::McpSurface;
use crate::project_roster::ProjectRole;
use imbl::OrdMap;
use std::collections::BTreeSet;
use std::sync::Arc;

#[derive(Clone, PartialEq, Eq)]
pub(in crate::app) struct IssuedCodingAddress {
    pub owner: String,
    pub agent: String,
    pub canonical_owner: String,
    pub conversation: String,
    pub thread_id: String,
    pub generation: u64,
}

#[derive(Clone, PartialEq, Eq)]
pub(in crate::app) struct FrozenMcpAddress {
    pub issued: IssuedCodingAddress,
    pub resident_floor: u64,
}

#[derive(Clone, PartialEq, Eq)]
struct AgentIdentity {
    id: String,
    binding: String,
    thread_id: String,
    generation: u64,
    resident_floor: u64,
    name: Option<String>,
    project_role: Option<ProjectRole>,
}

#[derive(Clone, PartialEq, Eq)]
struct OwnerIdentity {
    is_plan: bool,
    // AgentRoster::iter order is authoritative for omitted agent_id.
    agents: Vec<AgentIdentity>,
    pin: Option<String>,
}

#[derive(Clone, Default)]
pub(in crate::app) struct McpAddresses {
    owners: OrdMap<String, Arc<OwnerIdentity>>,
    agents: OrdMap<String, String>,
    order: Arc<Vec<String>>,
    deferred_topology: bool,
    has_collisions: bool,
}

impl OwnerIdentity {
    fn capture(app: &AppState, owner: &str) -> Option<Self> {
        let roster = app.entity_agents(owner).ok()?;
        let agents = roster
            .iter()
            .map(|agent| {
                #[cfg(test)]
                metrics::agent();
                AgentIdentity {
                    id: agent.id.clone(),
                    binding: agent.conversation_id().to_owned(),
                    thread_id: agent.thread.id.clone(),
                    generation: agent.thread.generation_revision,
                    resident_floor: agent.thread.resident_from_sequence(),
                    name: agent.name.clone(),
                    project_role: agent.project_role,
                }
            })
            .collect();
        Some(Self {
            is_plan: app.plans.contains_key(owner),
            agents,
            pin: app.project_pin(owner).map(str::to_owned),
        })
    }

    fn member_ids(&self) -> impl Iterator<Item = &str> {
        self.agents.iter().map(|agent| agent.id.as_str())
    }

    fn agent(&self, id: &str) -> Option<&AgentIdentity> {
        self.agents.iter().find(|agent| agent.id == id)
    }
}

impl McpAddresses {
    pub fn capture(app: &AppState) -> Self {
        let mut context = Self::default();
        for owner in app.plans.keys().chain(app.runs.keys()) {
            if let Some(record) = OwnerIdentity::capture(app, owner) {
                context.owners.insert(owner.clone(), Arc::new(record));
            }
        }
        context.reindex(app);
        context
    }

    pub fn update_owner(&mut self, app: &AppState, owner: &str) {
        let record = OwnerIdentity::capture(app, owner);
        let before = self.owners.get(owner);
        let topology_changed = match (before, &record) {
            (Some(before), Some(after)) => !before.member_ids().eq(after.member_ids()),
            (None, None) => false,
            _ => true,
        };
        if before.map(Arc::as_ref) != record.as_ref() {
            match record {
                Some(record) => {
                    self.owners.insert(owner.into(), Arc::new(record));
                }
                None => {
                    self.owners.remove(owner);
                }
            }
        }
        if topology_changed || self.deferred_topology || self.has_collisions {
            self.reindex(app);
        }
    }

    pub fn retain_owner_agents(&mut self, app: &AppState, owner: &str) {
        let Some(saved) = self.owners.get(owner) else {
            return;
        };
        let Ok(live) = app.entity_agents(owner) else {
            self.forget_owner(app, owner);
            return;
        };
        let mut retained = (**saved).clone();
        retained
            .agents
            .retain(|agent| live.by_id(&agent.id).is_some());
        let pin = retained.pin.take();
        retained.pin = pin.filter(|pin| retained.agent(pin).is_some());
        if &retained == saved.as_ref() {
            self.refresh_order(app);
            return;
        }
        self.owners.insert(owner.into(), Arc::new(retained));
        self.reindex(app);
    }

    pub fn forget_owner(&mut self, app: &AppState, owner: &str) {
        if self.owners.remove(owner).is_some() {
            self.reindex(app);
        }
    }

    pub fn refresh_order(&mut self, app: &AppState) {
        if self.has_collisions || self.deferred_topology {
            self.reindex(app);
        }
    }

    // Membership changes and rare colliding IDs reach this native-order pass.
    // Same-ID take/reinsert may reorder colliding owners; normal unique IDs
    // need no ranking refresh. Borrowed owners
    // retain their committed rank until the existing settlement barrier ends.
    fn reindex(&mut self, app: &AppState) {
        let visible = app
            .plans
            .keys()
            .chain(app.runs.keys())
            .filter(|owner| self.owners.contains_key(*owner))
            .cloned()
            .collect::<Vec<_>>();
        let has_borrowed = self
            .owners
            .keys()
            .any(|owner| !app.plans.contains_key(owner) && !app.runs.contains_key(owner));
        let order = if has_borrowed {
            let mut order = self
                .order
                .iter()
                .filter(|owner| self.owners.contains_key(*owner))
                .cloned()
                .collect::<Vec<_>>();
            let prior = order.iter().cloned().collect::<BTreeSet<_>>();
            order.extend(visible.into_iter().filter(|owner| !prior.contains(owner)));
            // Stable partition preserves prior within-kind rank while making
            // newly committed plans precede retained runs during borrowing.
            order.sort_by_key(|owner| !self.owners[owner].is_plan);
            order
        } else {
            visible
        };
        let mut agents = OrdMap::new();
        let mut has_collisions = false;
        for owner in &order {
            for agent in &self.owners[owner].agents {
                #[cfg(test)]
                metrics::indexed();
                if agents.contains_key(&agent.id) {
                    has_collisions = true;
                } else {
                    agents.insert(agent.id.clone(), owner.clone());
                }
            }
        }
        self.agents = agents;
        self.order = Arc::new(order);
        self.deferred_topology = has_borrowed;
        self.has_collisions = has_collisions;
    }

    fn agent(&self, owner: &str, id: Option<&str>) -> Result<&AgentIdentity, String> {
        let roster = self.owners.get(owner).ok_or("unknown id")?;
        match id {
            None => roster.agents.first().ok_or_else(|| NO_AGENT_YET.to_owned()),
            Some("") => Err("agent_id cannot be empty".into()),
            Some(id) => roster
                .agent(id)
                .ok_or_else(|| format!("unknown agent_id: {id}")),
        }
    }

    pub fn resolve(&self, owner: &str, id: Option<&str>) -> Result<FrozenMcpAddress, String> {
        let agent = self.agent(owner, id)?;
        let canonical_owner = self.agents.get(&agent.binding).ok_or_else(|| {
            format!(
                "agent {} is bound to missing conversation {}",
                agent.id, agent.binding
            )
        })?;
        let canonical = self.agent(canonical_owner, Some(&agent.binding))?;
        Ok(FrozenMcpAddress {
            issued: IssuedCodingAddress {
                owner: owner.into(),
                agent: agent.id.clone(),
                canonical_owner: canonical_owner.clone(),
                conversation: agent.binding.clone(),
                thread_id: canonical.thread_id.clone(),
                generation: canonical.generation,
            },
            resident_floor: canonical.resident_floor,
        })
    }

    pub fn issued_address(&self, agent: &str) -> Result<IssuedCodingAddress, String> {
        let owner = self.agents.get(agent).ok_or("unauthorized MCP session")?;
        Ok(self.resolve(owner, Some(agent))?.issued)
    }

    pub fn validate(&self, target: &IssuedCodingAddress) -> Result<(), String> {
        let current = self.resolve(&target.owner, Some(&target.agent))?.issued;
        if current.conversation != target.conversation {
            return Err(format!(
                "stale conversation_id {}; agent {} is bound to {}",
                target.conversation, target.agent, current.conversation
            ));
        }
        if current.thread_id != target.thread_id || current.generation != target.generation {
            return Err(format!(
                "stale thread_id {}; current thread is {}",
                target.thread_id, current.thread_id
            ));
        }
        if current.canonical_owner != target.canonical_owner {
            return Err("unauthorized MCP session".into());
        }
        Ok(())
    }

    pub fn surface(&self, owner: &str, agent: &str) -> Result<McpSurface, String> {
        self.agent(owner, Some(agent))?;
        Ok(if crate::router::is_router_agent(agent) {
            McpSurface::Router
        } else if self
            .owners
            .get(owner)
            .and_then(|record| record.pin.as_deref())
            == Some(agent)
        {
            McpSurface::Project
        } else {
            McpSurface::Coding
        })
    }

    pub fn is_pinned_coordinator(&self, agent: &str) -> bool {
        crate::agent::is_project_agent(agent)
            && self.agents.get(agent).is_some_and(|owner| {
                self.owners
                    .get(owner)
                    .and_then(|record| record.pin.as_deref())
                    == Some(agent)
            })
    }

    pub fn display_name(&self, agent: &str) -> Option<&str> {
        let owner = self.agents.get(agent)?;
        self.agent(owner, Some(agent)).ok()?.name.as_deref()
    }
}

impl AppState {
    pub(in crate::app) fn live_mcp_issued_address(
        &self,
        owner: &str,
        agent: &str,
    ) -> Result<IssuedCodingAddress, String> {
        let address = self.resolve_conversation_address(owner, Some(agent))?;
        self.guard_conversation_reservation(&address)?;
        let thread = self.conversation_at(&address)?;
        Ok(IssuedCodingAddress {
            owner: owner.into(),
            agent: address.agent_id.clone(),
            canonical_owner: address.conversation_entity_id.clone(),
            conversation: address.conversation_id.clone(),
            thread_id: thread.id.clone(),
            generation: thread.generation_revision,
        })
    }
}

#[cfg(test)]
pub(in crate::app) mod metrics {
    use std::cell::Cell;
    thread_local! { static COUNTS: Cell<(usize,usize)> = const { Cell::new((0,0)) }; }
    pub(super) fn agent() {
        COUNTS.with(|c| {
            let (a, b) = c.get();
            c.set((a + 1, b));
        });
    }
    pub(super) fn indexed() {
        COUNTS.with(|c| {
            let (a, b) = c.get();
            c.set((a, b + 1));
        });
    }
    pub fn measure<T>(work: impl FnOnce() -> T) -> (T, (usize, usize)) {
        COUNTS.with(|c| c.set((0, 0)));
        let result = work();
        (result, COUNTS.with(Cell::get))
    }
}
