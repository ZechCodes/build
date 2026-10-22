use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::app::runtime::sessions::TerminalCleanupCandidate;
use crate::app::{AppState, LifecycleDiagnostic, TabRole};

impl AppState {
    /// Capture registration identities briefly, then probe roots without the
    /// app guard. Each disappearance is a separate, generation-checked command.
    pub(in crate::app) fn reap_orphaned_terminals(state: &Arc<Mutex<Self>>) -> Vec<String> {
        Self::reap_orphaned_terminals_with_probe(state, Path::exists)
    }

    pub(in crate::app) fn reap_orphaned_terminals_with_probe(
        state: &Arc<Mutex<Self>>,
        mut root_exists: impl FnMut(&Path) -> bool,
    ) -> Vec<String> {
        let candidates = state
            .lock()
            .unwrap()
            .session_registry
            .terminal_cleanup_candidates();
        let vanished = candidates
            .into_iter()
            .filter(|candidate| !root_exists(&candidate.key().root));
        let mut reaped = Vec::new();
        for candidate in vanished {
            if let Some(wire_id) = state.lock().unwrap().reap_terminal_candidate(&candidate) {
                reaped.push(wire_id);
            }
        }
        reaped
    }

    /// The root's disappearance was observed by the caller. Never probe disk
    /// here: the registration fence decides whether that observation still
    /// describes this tab, including sessions without persisted lineage.
    pub(in crate::app) fn reap_terminal_candidate(
        &mut self,
        candidate: &TerminalCleanupCandidate,
    ) -> Option<String> {
        if !self
            .session_registry
            .cleanup_candidate_is_current(candidate)
        {
            return None;
        }
        match candidate {
            TerminalCleanupCandidate::Tab { key, role, .. } => {
                let provider_thread_id = role
                    .agent()
                    .and_then(|(owner, agent)| self.recorded_resume_id(owner, agent));
                let retired = self.session_registry.retire_tab(
                    key,
                    "reaped",
                    LifecycleDiagnostic {
                        event: "shutdown_requested",
                        origin: "tab_retirement",
                        reason: Some("reaped"),
                        operation_id: None,
                        provider_thread_id: provider_thread_id.as_deref(),
                        caller: Some(std::panic::Location::caller()),
                    },
                )?;
                // EOF can no longer find the removed registration. Close its
                // exact lineage and in-flight turn before publishing cleanup.
                let published_owner = retired.instance.is_some_and(|instance| {
                    self.record_agent_session_end(
                        &instance.entity_id,
                        &instance.agent_id,
                        &instance,
                    )
                });
                match role {
                    TabRole::Shell => self.note_terminals_at(&key.root),
                    TabRole::Agent { owner, .. } => {
                        self.publish_terminal_cleanup(owner, !published_owner)
                    }
                }
                Some(retired.wire_id)
            }
            TerminalCleanupCandidate::WaitingScreen { key, .. } => {
                let wire_id = self.session_registry.remove_waiting_screen(key, "reaped")?;
                if let Some(owner) = key
                    .tab_id
                    .strip_prefix("agent:")
                    .and_then(|agent_id| self.entity_of_agent(agent_id))
                {
                    self.publish_terminal_cleanup(&owner, true);
                }
                Some(wire_id)
            }
        }
    }

    /// Apply a confirmed lifecycle removal only to registrations captured
    /// before its filesystem work began.
    pub(in crate::app) fn reap_terminal_candidates_at(
        &mut self,
        candidates: &[TerminalCleanupCandidate],
        root: &Path,
    ) {
        for candidate in candidates
            .iter()
            .filter(|candidate| candidate.key().root == root)
        {
            self.reap_terminal_candidate(candidate);
        }
    }

    fn publish_terminal_cleanup(&self, owner: &str, refresh_snapshot: bool) {
        if refresh_snapshot {
            self.publish_owner_snapshot_projection(owner);
        }
        self.changes.note_entity(owner);
        self.publish_live_roster_for(owner);
    }
}
