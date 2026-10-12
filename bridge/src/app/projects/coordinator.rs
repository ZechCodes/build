//! A project's pinned coordinator and the workers below it, as the bridge
//! keeps them on the project's conversation owner ([`crate::project_roster`]).
//!
//! The bridge alone makes the coordinator, and nothing but deleting the
//! project retires it: no request may remove it, end its owner or hand its
//! purpose to another agent. Every other agent on the owner is a worker.
//! Prompts and tools follow the persisted purpose, never the `project-` prefix
//! every one of these agents shares.

use crate::agent::AgentRoster;
use crate::app::AppState;
use crate::mcp::McpSurface;
use crate::models::ModelChoice;
use crate::project_roster::{ProjectRole, ProjectRoster};
use crate::store::now_rfc3339;
use serde_json::Value;

/// The refusal for a request that would retire the project's pinned agent.
pub(crate) const PINNED_AGENT_STAYS: &str =
    "Build keeps the project's agent for as long as the project exists. Delete the project to retire it.";

/// The refusal for an `agent.add` that asks for any purpose but a worker's.
const ONLY_WORKERS_ARE_ADDED: &str =
    "agent.add: project_role must be \"worker\"; Build makes the project's own agent itself.";

/// The refusal for a purpose asked of an owner that is not a project's.
const NO_PROJECT_ROLE_HERE: &str =
    "agent.add: project_role must only name an agent of a project's conversation.";

impl AppState {
    /// The pinned coordinator of `owner`: its recorded pin, while that agent
    /// is on its roster as the coordinator. Read off the record alone — only
    /// a project's owner is ever given a pin — so no state of the project's
    /// folder can take the pin's purpose away.
    pub(in crate::app) fn project_pin(&self, owner: &str) -> Option<&str> {
        let active = self.runs.get(owner)?;
        let pinned = active.pinned_agent_id.as_deref()?;
        active
            .agents
            .by_id(pinned)
            .filter(|agent| agent.project_role == Some(ProjectRole::Coordinator))
            .map(|agent| agent.id.as_str())
    }

    /// Whether `agent_id` is the pinned coordinator of `owner`.
    pub(in crate::app) fn is_project_coordinator(&self, owner: &str, agent_id: &str) -> bool {
        self.project_pin(owner) == Some(agent_id)
    }

    /// Whether `agent_id` is the pinned coordinator of whichever owner holds
    /// it: for the callers that know an agent and not its owner.
    pub(in crate::app) fn is_pinned_coordinator(&self, agent_id: &str) -> bool {
        crate::agent::is_project_agent(agent_id)
            && self
                .entity_of_agent(agent_id)
                .is_some_and(|owner| self.is_project_coordinator(&owner, agent_id))
    }

    /// The Build MCP surface a session of `agent_id` on `owner` is on, read
    /// off the agent's persisted purpose: a router's session is the router's,
    /// a project's pinned coordinator is on the project surface, and every
    /// other agent — a project worker included — is on the coding surface.
    pub(in crate::app) fn agent_mcp_surface(&self, owner: &str, agent_id: &str) -> McpSurface {
        if crate::router::is_router_agent(agent_id) {
            McpSurface::Router
        } else if self.is_project_coordinator(owner, agent_id) {
            McpSurface::Project
        } else {
            McpSurface::Coding
        }
    }

    /// Give a project's conversation owner its one pinned coordinator, and
    /// answer it.
    ///
    /// A roster written before pins existed is settled first: its primary
    /// becomes the pin and every other agent a worker, keeping their ids,
    /// history and settings. Only when there is no pin to keep is a fresh
    /// coordinator minted, on `asked` or the device's project-agent choice.
    /// Written only when something changed, so asking again is free.
    pub(in crate::app) fn ensure_project_pin(
        &mut self,
        owner: &str,
        asked: Option<ModelChoice>,
    ) -> Result<String, String> {
        let choice = match asked {
            Some(asked) => asked,
            None => self.mint_model_choice(owner)?,
        };
        let kind = self.agent_owner(owner);
        let mut active = self.take_run(owner)?;
        let mut roster = ProjectRoster::new(&mut active.pinned_agent_id, &mut active.agents);
        let changed = roster.settle();
        if let Some(pin) = roster.pin().map(str::to_string) {
            if changed {
                self.finish_run_mutation(owner.to_string(), active)?;
            } else {
                // Nothing moved, so the run goes back exactly as it came.
                self.runs.insert(owner.to_string(), active);
            }
            return Ok(pin);
        }
        let pin = mint_pin(
            &mut active.pinned_agent_id,
            &mut active.agents,
            kind,
            choice,
        );
        self.finish_run_mutation(owner.to_string(), active)?;
        Ok(pin)
    }

    /// Settle every live project conversation owner a boot restored, so a
    /// restarted bridge answers with exactly one stable pin per project.
    pub(in crate::app) fn settle_project_rosters(&mut self) {
        let owners: Vec<String> = self
            .runs
            .iter()
            .filter(|(_, active)| !active.run.state.is_terminal())
            .map(|(run_id, _)| run_id.clone())
            .filter(|run_id| self.is_project_conversation_owner(run_id))
            .collect();
        for owner in owners {
            if let Err(error) = self.ensure_project_pin(&owner, None) {
                eprintln!("settle project roster {owner}: {error}");
            }
        }
    }

    /// The purpose an `agent.add` on `entity_id` makes: a worker on a
    /// project's conversation owner, nothing anywhere else. Only the bridge
    /// makes a coordinator, so asking for one — or for a purpose on any other
    /// owner — is refused before anything is made.
    pub(in crate::app) fn added_project_role(
        &self,
        entity_id: &str,
        params: &Value,
    ) -> Result<Option<ProjectRole>, String> {
        let asked = params.get("project_role").filter(|value| !value.is_null());
        let on_project = self.is_project_conversation_owner(entity_id);
        match (
            asked.map(|value| value.as_str().and_then(ProjectRole::from_wire)),
            on_project,
        ) {
            (None, true) | (Some(Some(ProjectRole::Worker)), true) => Ok(Some(ProjectRole::Worker)),
            (None, false) => Ok(None),
            (Some(_), false) => Err(NO_PROJECT_ROLE_HERE.to_string()),
            (Some(_), true) => Err(ONLY_WORKERS_ARE_ADDED.to_string()),
        }
    }

    /// Refuse a request that would take the project's pinned agent away.
    pub(in crate::app) fn refuse_retiring_pin(
        &self,
        owner: &str,
        agent_id: &str,
    ) -> Result<(), String> {
        if self.is_project_coordinator(owner, agent_id) {
            return Err(PINNED_AGENT_STAYS.to_string());
        }
        Ok(())
    }

    /// Refuse a request that would end a project's conversation owner, and
    /// with it the pinned agent. Deleting the project is the one exit.
    pub(in crate::app) fn refuse_ending_project_owner(&self, run_id: &str) -> Result<(), String> {
        if self.is_project_conversation_owner(run_id) {
            return Err(PINNED_AGENT_STAYS.to_string());
        }
        Ok(())
    }

    /// Refuse to start a project worker whose project folder is gone.
    ///
    /// The pinned coordinator only reads, so it falls back to its scratch root
    /// when the folder is missing. A worker writes: started anywhere but the
    /// project's sources it would write into Build's own scratch, so it does
    /// not start, and the refusal is its start error.
    pub(in crate::app) fn refuse_worker_without_sources(
        &self,
        owner: &str,
        agent_id: &str,
    ) -> Result<(), String> {
        if !self.is_project_conversation_owner(owner)
            || self.is_project_coordinator(owner, agent_id)
        {
            return Ok(());
        }
        // The record's path when no project is bound: a boot that could not
        // reach the folder bound none, and the folder is still missing.
        let project_path = self.projects.project_path_for(owner);
        if std::path::Path::new(&project_path).is_dir() {
            return Ok(());
        }
        Err(format!(
            "Build cannot start this agent: the project folder {project_path} is missing."
        ))
    }

    /// The agents a roster read shows of `entity_id`. A project's owner shows
    /// only its pin unless the caller asked for its workers too, so a client
    /// that predates workers never paints one as a second project agent.
    pub(in crate::app) fn listed_agents(
        &self,
        entity_id: &str,
        include_project_workers: bool,
    ) -> Vec<&crate::agent::Agent> {
        let Ok(roster) = self.entity_agents(entity_id) else {
            return Vec::new();
        };
        if include_project_workers || !self.is_project_conversation_owner(entity_id) {
            return roster.iter().collect();
        }
        let pin = self.project_pin(entity_id);
        roster
            .iter()
            .filter(|agent| Some(agent.id.as_str()) == pin)
            .collect()
    }
}

/// Pin a fresh coordinator on a project owner about to be stored for the
/// first time.
pub(super) fn pin_new_owner(
    run_id: &str,
    active: &mut crate::orchestrator::ActiveRun,
    choice: ModelChoice,
) {
    mint_pin(
        &mut active.pinned_agent_id,
        &mut active.agents,
        crate::agent::AgentOwner::project(run_id),
        choice,
    );
}

/// Mint a coordinator onto a project owner's roster and pin it.
fn mint_pin(
    pinned: &mut Option<String>,
    agents: &mut AgentRoster,
    kind: crate::agent::AgentOwner<'_>,
    choice: ModelChoice,
) -> String {
    let pin = agents.add(kind, choice, &now_rfc3339()).id.clone();
    ProjectRoster::new(pinned, agents).pin_minted(&pin);
    pin
}
