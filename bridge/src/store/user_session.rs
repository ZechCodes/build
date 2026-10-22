//! The user's own session (spec: Tasks dashboard → Done since you left).
//!
//! One row in `meta`, rewritten whenever the user acts, and the replay a boot
//! folds it together with: every stored action the user took, by timestamp.

use super::{Store, StoreError};
use crate::session_summary::{message_millis, UserSession};
use rusqlite::OptionalExtension;
use std::path::PathBuf;

const USER_SESSION_KEY: &str = "user_session";

impl Store {
    pub fn load_user_session(&self) -> Result<Option<UserSession>, StoreError> {
        let raw: Option<String> = self
            .connection()
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                [USER_SESSION_KEY],
                |row| row.get(0),
            )
            .optional()?;
        raw.map(|raw| {
            serde_json::from_str(&raw).map_err(|source| StoreError::Corrupt {
                path: PathBuf::from(format!("meta/{USER_SESSION_KEY}")),
                source,
            })
        })
        .transpose()
    }

    pub fn save_user_session(&self, session: &UserSession) -> Result<(), StoreError> {
        let value = serde_json::to_string(session).expect("a user session serializes");
        self.connection().execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![USER_SESSION_KEY, value],
        )?;
        Ok(())
    }

    /// Save a presence snapshot captured before releasing application state.
    /// Another action may have already persisted a newer session while this
    /// caller waited for the writer. With the same latest activity, observations
    /// can only move the session start earlier and the preceding end later.
    pub fn save_user_session_if_newer(&self, session: &UserSession) -> Result<(), StoreError> {
        let Some(last_activity) = session.last_activity_ms else {
            return Ok(());
        };
        let value = serde_json::to_string(session).expect("a user session serializes");
        self.connection().execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value \
             WHERE json_extract(meta.value, '$.last_activity_ms') IS NULL \
                OR json_extract(meta.value, '$.last_activity_ms') < ?3 \
                OR (json_extract(meta.value, '$.last_activity_ms') = ?3 \
                    AND (json_extract(meta.value, '$.session_started_ms') IS NULL \
                         OR ?4 <= json_extract(meta.value, '$.session_started_ms')) \
                    AND (json_extract(meta.value, '$.previous_session_ended_ms') IS NULL \
                         OR ?5 >= json_extract(meta.value, '$.previous_session_ended_ms')))",
            rusqlite::params![
                USER_SESSION_KEY,
                value,
                last_activity,
                session.session_started_ms,
                session.previous_session_ended_ms,
            ],
        )?;
        Ok(())
    }

    /// When the user did each stored thing: the task events and comments
    /// they wrote, and the messages they sent. Oldest first. A message an
    /// agent sent, or Build wrote, is not the user's however it is addressed.
    pub fn user_action_times(&self) -> Result<Vec<i64>, StoreError> {
        let connection = self.connection();
        let mut times = Vec::new();
        for query in [
            "SELECT at FROM tracker_events \
             WHERE json_extract(record, '$.actor.kind') = 'user'",
            "SELECT created_at FROM tracker_comments \
             WHERE json_extract(record, '$.author.kind') = 'user'",
            "SELECT json_extract(item, '$.data.created_at') FROM thread_items \
             WHERE message = 1 \
               AND json_extract(item, '$.data.role') = 'user' \
               AND json_extract(item, '$.data.from_agent') IS NULL \
               AND COALESCE(json_extract(item, '$.data.from_build'), 0) = 0 \
               AND json_extract(item, '$.data.task_notice') IS NULL",
        ] {
            let mut statement = connection.prepare(query)?;
            let rows = statement.query_map([], |row| row.get::<_, Option<String>>(0))?;
            for at in rows {
                times.extend(at?.as_deref().and_then(message_millis));
            }
        }
        times.sort_unstable();
        Ok(times)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_summary::USER_SESSION_GAP_MS;

    #[test]
    fn delayed_terminal_presence_preserves_newer_activity_and_session_boundaries() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::new(directory.path()).unwrap();
        let opened = UserSession::default().updated(100);
        store.save_user_session_if_newer(&opened).unwrap();
        assert_eq!(store.load_user_session().unwrap(), Some(opened));

        // Frames sample their timestamps before acquiring application state:
        // an older observation may move the start back after a later one saved.
        let earlier_start = opened.updated(90);
        assert_eq!(earlier_start.last_activity_ms, opened.last_activity_ms);
        store.save_user_session_if_newer(&earlier_start).unwrap();
        assert_eq!(store.load_user_session().unwrap(), Some(earlier_start));
        store.save_user_session_if_newer(&opened).unwrap();
        assert_eq!(store.load_user_session().unwrap(), Some(earlier_start));

        let later = opened.updated(200 + USER_SESSION_GAP_MS);
        store.save_user_session(&later).unwrap();
        store.save_user_session_if_newer(&opened).unwrap();
        assert_eq!(store.load_user_session().unwrap(), Some(later));

        let boundary = later.updated(150);
        assert_ne!(boundary, later);
        assert_eq!(boundary.last_activity_ms, later.last_activity_ms);
        store.save_user_session_if_newer(&boundary).unwrap();
        assert_eq!(store.load_user_session().unwrap(), Some(boundary));
        store.save_user_session_if_newer(&later).unwrap();
        assert_eq!(store.load_user_session().unwrap(), Some(boundary));

        let latest = boundary.updated(300 + USER_SESSION_GAP_MS);
        store.save_user_session_if_newer(&latest).unwrap();
        assert_eq!(store.load_user_session().unwrap(), Some(latest));
    }
}
