use super::AppState;
use crate::orchestrator::ActiveRun;
use crate::session_summary::{message_millis, SessionSummary, UserSession};
use crate::thread::ThreadItem;

impl AppState {
    /// Observe only newly appended words. An in-place edit of an older item
    /// keeps its creation sequence and therefore cannot move an inbox row.
    pub(in crate::app) fn note_run_messages(&mut self, run_id: &str, active: &ActiveRun) {
        let project_id = self.projects.project_id_of(run_id).map(str::to_owned);
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
                self.note_owner_message(run_id, project_id.as_deref(), ts);
            }
            self.session_seen.insert(key, newest);
        }
    }

    /// Every conversation owner keeps its own summary — the project's own
    /// conversation included, whose board row the inbox orders by it (#103)
    /// — and the project pools them all.
    fn note_owner_message(&mut self, owner_id: &str, project_id: Option<&str>, ts: i64) {
        let summary = self
            .session_summaries
            .entry(owner_id.to_owned())
            .or_default();
        *summary = summary.updated(ts);
        if let Some(project_id) = project_id {
            let summary = self
                .session_summaries
                .entry(project_id.to_owned())
                .or_default();
            *summary = summary.updated(ts);
        }
    }

    /// A deleted conversation, or a deleted project, keeps no summary: the
    /// map lives as long as the bridge does.
    pub(in crate::app) fn forget_session_owner(&mut self, owner_id: &str) {
        self.session_summaries.remove(owner_id);
        self.session_seen.retain(|(owner, _), _| owner != owner_id);
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
                self.note_owner_message(&owner, project_id.as_deref(), ts);
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

    /// The user did something, now. Only the verbs that are the user acting
    /// call this, so an agent working through the night never starts a
    /// session. A write that fails is logged rather than failing the verb:
    /// the next action rewrites the whole row, and a boot replays the actions
    /// the store already holds.
    ///
    /// A new session is pushed to every client as an `issues` change on every
    /// project, because the list answer carries the session: a laptop holding
    /// the old one reads the new start rather than inferring an absence the
    /// user spent on their phone. Activity inside a session pushes nothing;
    /// no client decides anything from `last_activity_ms` alone for long.
    pub(in crate::app) fn note_user_activity(&mut self, ts: i64) {
        let before = self.user_session;
        let updated = before.updated(ts);
        if updated == before {
            return;
        }
        self.user_session = updated;
        if let Some(store) = self.store.as_ref() {
            if let Err(error) = store.save_user_session(&updated) {
                eprintln!("user session persist failed: {error}");
            }
        }
        let new_boundary = updated.session_started_ms != before.session_started_ms
            || updated.previous_session_ended_ms != before.previous_session_ended_ms;
        if new_boundary {
            for project in self.projects.iter() {
                self.changes.note_issues(&project.id, &[]);
            }
        }
    }

    #[cfg(test)]
    pub(in crate::app) fn user_session(&self) -> UserSession {
        self.user_session
    }

    /// The session as `issues.list` and `user.present` answer it, with the
    /// bridge's clock beside it: a client measures the six-hour silence and
    /// the 96-hour absence against this, never against its own clock.
    pub(in crate::app) fn user_session_json(&self) -> serde_json::Value {
        let session = self.user_session;
        serde_json::json!({
            "session_started_ms": session.session_started_ms,
            "last_activity_ms": session.last_activity_ms,
            "previous_session_ended_ms": session.previous_session_ended_ms,
            "gap_ms": crate::session_summary::USER_SESSION_GAP_MS,
            "now_ms": i64::try_from(crate::agent::now_ms()).unwrap_or(i64::MAX),
        })
    }

    /// `user.present` — the user arrived at a client: its window came to the
    /// front, or they touched or navigated it. Recorded on this bridge's
    /// clock, so a reload, or another client, reads the same arrival.
    pub(crate) fn user_present(&mut self) -> serde_json::Value {
        let now = i64::try_from(crate::agent::now_ms()).unwrap_or(i64::MAX);
        self.note_user_activity(now);
        serde_json::json!({ "user_session": self.user_session_json() })
    }

    /// The persisted summary, then every stored user action after it.
    ///
    /// The summary is authoritative for the interval it covers: it is
    /// rewritten on every change, and much of what made it (read marks, which
    /// keep only their latest) is not in the store to replay. Replaying only
    /// its endpoints would split a session sustained by read marks. The
    /// actions after its last activity are the ones a failed write missed; a
    /// store with no summary (a bridge from before this shipped) replays them
    /// all.
    pub(in crate::app) fn rebuild_user_session(&mut self) -> Result<(), String> {
        let Some(store) = self.store.as_ref() else {
            return Ok(());
        };
        let times = store
            .user_action_times()
            .map_err(|error| format!("user session rebuild: {error}"))?;
        let saved = store
            .load_user_session()
            .map_err(|error| format!("user session rebuild: {error}"))?
            .unwrap_or_default();
        let replay_after = saved.last_activity_ms.unwrap_or(i64::MIN);
        self.user_session = times
            .into_iter()
            .filter(|ts| *ts > replay_after)
            .fold(saved, UserSession::updated);
        if self.user_session != saved {
            store
                .save_user_session(&self.user_session)
                .map_err(|error| format!("user session rebuild: {error}"))?;
        }
        Ok(())
    }
}
