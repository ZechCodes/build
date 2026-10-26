//! Watched tasks as inbox rows (spec: Tasks → Watching).
//!
//! The user watches a task and it appears in the inbox beside the
//! conversations, ordered by when it last moved. One row per watched task,
//! carrying what the row draws and nothing else: the task itself is a click
//! away, and a row that carried the body would be the inbox re-rendering the
//! task page in miniature.
//!
//! The row's subtitle is the reader-facing voice of the same line an agent
//! gets as a notice ([`super::notices::notice_line`]). One composer, two
//! audiences: the facts cannot drift apart because there is one place that
//! decides them.

use super::notices::{notice_line, NoticeVoice};
use crate::app::AppState;
use crate::store::TrackerTaskObservation;
use crate::tracker::summary::{after, attention_bookkeeping, entry_actor, entry_at};
pub(crate) use crate::tracker::summary::{
    entry_id, event_counts_as_unread, task_activity_session, unread_since_mark, when,
};
use crate::tracker::{Assignee, Task, TimelineEntry};
use serde_json::{json, Value};

impl AppState {
    /// Every watched task, as inbox rows.
    ///
    /// Rows for tasks only: the conversations beside them are the board's own
    /// and are built where they always were. They land in the same list so the
    /// interleave by `anchor` costs nothing on either side.
    #[cfg(test)]
    pub(in crate::app) fn watched_task_rows(&self) -> Vec<Value> {
        let projects: Vec<String> = self
            .projects
            .iter()
            .map(|project| project.id.clone())
            .collect();
        projects
            .into_iter()
            .flat_map(|project| {
                self.watched_task_rows_for_project(&project)
                    .expect("watched task fixture is readable")
            })
            .collect()
    }

    pub(in crate::app) fn watched_task_rows_for_project(
        &self,
        project_id: &str,
    ) -> Result<Vec<Value>, String> {
        let Some(store) = &self.store else {
            return Ok(Vec::new());
        };
        let path = self.tracker_project_path(project_id)?;
        let observations = store
            .watched_tracker_task_observations(&path)
            .map_err(|error| error.to_string())?;
        Ok(observations
            .iter()
            .filter_map(|observation| self.watched_task_observation_row(project_id, observation))
            .collect())
    }

    /// Pure row construction from one committed transaction. An absent row
    /// means unwatching or no meaningful activity, never a failed SQL read.
    pub(in crate::app) fn watched_task_observation_row(
        &self,
        project_id: &str,
        observation: &TrackerTaskObservation,
    ) -> Option<Value> {
        let task = &observation.task;
        if !task.watched {
            return None;
        }
        let timeline = &observation.timeline;
        let last = timeline
            .iter()
            .rev()
            .find(|entry| !attention_bookkeeping(entry))?;
        let at = entry_at(last).to_string();
        let session = task_activity_session(task, timeline);
        let blocked = observation.dependencies.blocked;
        let assigned_to_user = matches!(task.assignee, Some(Assignee::User)) && !blocked;
        let mut row = json!({
            "kind": "tracker_task",
            "task_id": task.id,
            "number": task.number,
            "project_id": project_id,
            "title": task.title,
            "status": task.status,
            "assignee": task.assignee,
            "assigned_to_user": assigned_to_user,
            "last_event": {
                "text": self.reader_line_for(task, last),
                "actor": self.observed_task_actor_words(task, entry_actor(last)).to_reader,
                "at": at,
            },
            // The two the inbox interleaves by. Both the same instant: a row's
            // anchor IS when it last moved, and the existing rows send both.
            "anchor": at,
            "last_activity": at,
            "session_started_ms": session.session_started_ms,
            "last_activity_ms": session.last_activity_ms,
            "unread": unread_since_mark(task, timeline),
            // Mute is unwatch here, so a row that exists is not muted — the
            // absence of the row is the whole of the answer.
            "muted": false,
            "done_until_next": self.task_is_done_until_next(task, timeline),
        });
        if let Some(summary) = &observation.review {
            row["review_summary"] = json!(summary);
        }
        extend_observed_waiting_summary(observation, &mut row);
        Some(row)
    }

    /// What the row's subtitle says: the reader's voice of the same line an
    /// agent would be sent about this event.
    fn reader_line_for(&self, task: &Task, entry: &TimelineEntry) -> String {
        let Some(notice) = super::notices::notice_of_entry(entry) else {
            // A timeline entry the notice vocabulary has no word for — a
            // tracking change, today. The row still needs a subtitle, and
            // when it last moved is the honest one.
            return "Updated".to_string();
        };
        notice_line(
            &notice,
            task,
            &self.observed_task_actor_words(task, entry_actor(entry)),
            NoticeVoice::Reader,
        )
    }

    /// Whether the user cleared this row and nothing has happened since.
    fn task_is_done_until_next(&self, task: &Task, timeline: &[TimelineEntry]) -> bool {
        let Some(cleared) = task.dismissed_through.as_deref() else {
            return false;
        };
        !timeline
            .iter()
            .filter(|entry| !attention_bookkeeping(entry))
            .any(|entry| after(Some(cleared), entry_id(entry)))
    }
}

pub(super) fn extend_observed_waiting_summary(
    observation: &TrackerTaskObservation,
    row: &mut Value,
) {
    row["dependencies"] = json!(observation.dependencies);
    row["dependencies"]["blocks"] = json!(observation.blocks.blocks);
    if let Some(cursor) = &observation.blocks.next_cursor {
        row["dependencies"]["blocks_next_cursor"] = json!(cursor);
    }
    if let Some(intent) = observation.delivery.as_ref().filter(|intent| {
        matches!(
            intent.state.as_str(),
            "waiting" | "queued" | "provisioning" | "failed"
        )
    }) {
        let state = match intent.state.as_str() {
            "provisioning" => "queued",
            state => state,
        };
        row["deferred_assignment"] =
            json!({"id":intent.id,"target":intent.target,"state":state,"error":intent.error});
    }
}
