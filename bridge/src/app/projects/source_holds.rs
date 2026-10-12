//! A project's live workers hold its folders in place.
//!
//! A worker may stand in, and write to, any of its project's sources, so
//! while one is running — or being started, or has a turn queued to start
//! it — none of them may move or leave the project. The refusal names who
//! must stop first; nothing here stops anybody. The other direction holds
//! too: while a source is moving, no worker is started into the folders
//! that are changing under it. This is lifecycle consistency, not isolation.

use crate::app::{AppState, TabKey};
use crate::project_roster::ProjectRole;
use std::collections::HashSet;

/// Project ids whose source is moving right now, from admission to the
/// settlement that writes (or refuses) the new folder.
#[derive(Debug, Default)]
pub(in crate::app) struct SourceMoves(HashSet<String>);

impl SourceMoves {
    pub(in crate::app) fn begin(&mut self, project_id: &str) {
        self.0.insert(project_id.to_string());
    }

    pub(in crate::app) fn end(&mut self, project_id: &str) {
        self.0.remove(project_id);
    }

    fn moving(&self, project_id: &str) -> bool {
        self.0.contains(project_id)
    }
}

impl AppState {
    /// The workers of `project_id` that hold its sources: a session that is
    /// live, one being started, or a turn that will start one. Named the way
    /// the user knows them.
    fn workers_holding_sources(&self, project_id: &str) -> Vec<String> {
        let Some(owner) = self.project_conversation_run(project_id) else {
            return Vec::new();
        };
        let (Ok(roster), Ok(root)) = (self.entity_agents(&owner), self.entity_agent_root(&owner))
        else {
            return Vec::new();
        };
        let root = Self::canonical_root(&root);
        roster
            .iter()
            .filter(|agent| agent.project_role == Some(ProjectRole::Worker))
            .filter(|agent| self.agent_session_held(&TabKey::agent(&root, &agent.id)))
            .map(|agent| agent.name.clone().unwrap_or_else(|| agent.id.clone()))
            .collect()
    }

    /// Whether the session at `key` is running, being started, or owed a
    /// queued or in-flight turn that would start it.
    fn agent_session_held(&self, key: &TabKey) -> bool {
        self.session_registry.agent_snapshot(key).is_some()
            || self.session_registry.claim_is_held(key)
            || self.delivery_queue.holds_agent(key)
    }

    /// Refuse to move or remove any of `project_id`'s sources while one of
    /// its workers holds them, naming the workers to stop first.
    pub(in crate::app) fn refuse_moving_sources_under_workers(
        &self,
        project_id: &str,
    ) -> Result<(), String> {
        let holding = self.workers_holding_sources(project_id);
        if holding.is_empty() {
            return Ok(());
        }
        Err(format!(
            "Build cannot move or remove this project's folders while its workers are working in them. Stop {} first.",
            holding.join(", ")
        ))
    }

    /// Refuse to start a project worker while one of its project's sources
    /// is moving. The move holds the folders until it settles.
    pub(in crate::app) fn refuse_worker_during_source_move(
        &self,
        owner: &str,
        agent_id: &str,
    ) -> Result<(), String> {
        let is_worker = self
            .entity_agents(owner)
            .ok()
            .and_then(|roster| roster.by_id(agent_id))
            .is_some_and(|agent| agent.project_role == Some(ProjectRole::Worker));
        let moving = self
            .projects
            .project_id_of(owner)
            .is_some_and(|project_id| self.source_moves.moving(project_id));
        if is_worker && moving {
            return Err(
                "Build cannot start this agent while one of its project's folders is moving. Start it once the move finishes."
                    .to_string(),
            );
        }
        Ok(())
    }
}
