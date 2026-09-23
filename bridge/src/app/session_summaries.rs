use super::AppState;
use crate::orchestrator::ActiveRun;
use crate::session_summary::{message_millis, SessionSummary};
use crate::thread::ThreadItem;

impl AppState {
    /// Observe only newly appended words. An in-place edit of an older item
    /// keeps its creation sequence and therefore cannot move an inbox row.
    pub(in crate::app) fn note_run_messages(&mut self, run_id: &str, active: &ActiveRun) {
        let project_id = self.projects.project_id_of(run_id).map(str::to_owned);
        let project_owner = project_id
            .as_deref()
            .and_then(|id| self.projects.get(id))
            .is_some_and(|project| {
                crate::app::workspaces::same_path(
                    &active.worktree.path,
                    &crate::app::projects::scratch_dir(&self.state_root, &project.repo_path),
                )
            });
        for agent in active.agents.agents() {
            let key = (run_id.to_owned(), agent.id.clone());
            let seen = self.session_seen.get(&key).copied().unwrap_or(0);
            let mut newest = seen;
            let mut fresh = Vec::new();
            for item in agent.thread.items.iter().rev() {
                if item.sequence() <= seen {
                    break;
                }
                newest = newest.max(item.sequence());
                if let ThreadItem::Message(message) = item {
                    if !message.from_build {
                        if let Some(ts) = message_millis(&message.created_at) {
                            fresh.push(ts);
                        }
                    }
                }
            }
            for ts in fresh.into_iter().rev() {
                self.note_owner_message(run_id, project_id.as_deref(), project_owner, ts);
            }
            self.session_seen.insert(key, newest);
        }
    }

    fn note_owner_message(
        &mut self,
        owner_id: &str,
        project_id: Option<&str>,
        project_owner: bool,
        ts: i64,
    ) {
        if !project_owner {
            let summary = self
                .session_summaries
                .entry(owner_id.to_owned())
                .or_default();
            *summary = summary.updated(ts);
        }
        if let Some(project_id) = project_id {
            let summary = self
                .session_summaries
                .entry(project_id.to_owned())
                .or_default();
            *summary = summary.updated(ts);
        }
    }

    pub(in crate::app) fn session_summary(&self, id: &str) -> SessionSummary {
        self.session_summaries.get(id).copied().unwrap_or_default()
    }

    /// The store's message index, ordered by timestamp, repairs any late
    /// message that could have connected to a session older than the live
    /// summary held. Mapping is ready after runs and projects are restored.
    pub(in crate::app) fn rebuild_session_summaries(&mut self) -> Result<(), String> {
        let Some(store) = self.store.as_ref() else {
            return Ok(());
        };
        let rows = store
            .session_message_times()
            .map_err(|error| format!("session rebuild: {error}"))?;
        self.session_summaries.clear();
        self.session_seen.clear();
        for (owner, retained_project_id, agent, sequence, ts) in rows {
            if let Some(owner) = owner {
                let project_id = self.projects.project_id_of(&owner).map(str::to_owned);
                let project_owner = self.is_project_conversation_owner(&owner);
                self.note_owner_message(&owner, project_id.as_deref(), project_owner, ts);
                self.session_seen
                    .entry((owner, agent))
                    .and_modify(|seen| *seen = (*seen).max(sequence))
                    .or_insert(sequence);
            } else if let Some(project_id) = retained_project_id {
                self.session_summaries
                    .entry(project_id)
                    .and_modify(|summary| *summary = summary.updated(ts))
                    .or_insert_with(|| SessionSummary::default().updated(ts));
            }
        }
        Ok(())
    }
}
