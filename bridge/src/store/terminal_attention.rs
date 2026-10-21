//! A delayed terminal stamp merges only its interaction pair into the current
//! record. It cannot replace newer seen, mute, dismissal, or read-cursor state.
use super::{Store, StoreError};
use crate::attention::Attention;
use rusqlite::OptionalExtension;

impl Store {
    pub(crate) fn terminal_attention_epoch(&self) -> u64 {
        self.attention_map_epoch
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    pub(crate) fn save_terminal_interaction(
        &self,
        owner: &str,
        last: &str,
        resume: Option<&str>,
        durable_owner: bool,
        external_epoch: Option<u64>,
    ) -> Result<(), StoreError> {
        self.in_transaction(|transaction| {
            if durable_owner
                && !transaction.query_row(
                    "SELECT EXISTS(SELECT 1 FROM implementations WHERE id = ?1) \
                 OR EXISTS(SELECT 1 FROM tasks WHERE id = ?1)",
                    [owner],
                    |row| row.get::<_, bool>(0),
                )?
            {
                return Ok(());
            }
            let current: Option<String> = transaction
                .query_row(
                    "SELECT record FROM attention WHERE entity_id = ?1",
                    [owner],
                    |row| row.get(0),
                )
                .optional()?;
            // Managed workspace IDs were excluded by the old live-map save.
            // External IDs may first insert only against their captured epoch.
            if current.is_none() && !durable_owner && external_epoch.is_none() {
                return Ok(());
            }
            let mut current: Attention = current
                .as_deref()
                .and_then(|value| serde_json::from_str(value).ok())
                .unwrap_or_default();
            let parse = |value: &str| {
                time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
                    .ok()
            };
            let Some(incoming) = parse(last) else {
                return Ok(());
            };
            if current
                .last_interaction_at
                .as_deref()
                .and_then(parse)
                .is_some_and(|held| held >= incoming)
            {
                return Ok(());
            }
            // Replaying interact(last) here would invent a new resume boundary
            // when two asynchronous stamps reach SQLite in reverse order.
            current.last_interaction_at = Some(last.to_owned());
            current.resume_at = resume.map(str::to_owned);
            transaction.execute(
                "INSERT INTO attention (entity_id, record) VALUES (?1, ?2) \
                 ON CONFLICT(entity_id) DO UPDATE SET record = excluded.record",
                rusqlite::params![
                    owner,
                    serde_json::to_string(&current).expect("attention serializes")
                ],
            )?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_interaction_merge_preserves_boundaries_and_does_not_create_aliases() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::new(directory.path()).unwrap();
        let first = "2026-01-02T00:00:00Z";
        let second = "2026-01-02T01:00:00Z";
        store
            .save_terminal_interaction("alias", second, Some(first), false, None)
            .unwrap();
        assert!(!store.load_attention().contains_key("alias"));
        let mut original = Attention::default();
        original.interact("2026-01-01T00:00:00Z");
        original.muted = true;
        original.seen_state_at = Some("seen-before-save".into());
        store
            .save_attention(
                &std::collections::HashMap::from([("alias".into(), original.clone())]),
                &std::collections::HashSet::from(["alias".into()]),
            )
            .unwrap();
        store
            .save_terminal_interaction("alias", second, Some(first), false, None)
            .unwrap();
        store
            .save_terminal_interaction("alias", first, Some(first), false, None)
            .unwrap();
        let saved = store.load_attention().remove("alias").unwrap();
        assert_eq!(saved.last_interaction_at.as_deref(), Some(second));
        assert_eq!(saved.resume_at.as_deref(), Some(first));
        assert_eq!(saved.seen_state_at, original.seen_state_at);
        assert!(saved.muted);
    }

    #[test]
    fn terminal_interaction_external_insert_is_fenced_by_live_map_pruning() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::new(directory.path()).unwrap();
        let stamp = "2026-01-02T00:00:00Z";
        let captured = store.terminal_attention_epoch();
        let delayed = store.clone();
        store
            .save_terminal_interaction("external", stamp, Some(stamp), false, Some(captured))
            .unwrap();
        assert_eq!(
            store.load_attention()["external"]
                .last_interaction_at
                .as_deref(),
            Some(stamp)
        );
        store
            .save_attention(&Default::default(), &Default::default())
            .unwrap();
        delayed
            .save_terminal_interaction("external", stamp, Some(stamp), false, Some(captured))
            .unwrap();
        assert!(!store.load_attention().contains_key("external"));
    }

    #[test]
    fn terminal_interaction_merge_requires_a_live_durable_owner() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::new(directory.path()).unwrap();
        let stamp = "2026-01-02T00:00:00Z";
        store.writer_for_test().execute(
            "INSERT INTO implementations(id, created_at, updated_at, record) VALUES('run', '', '', '{}')", [],
        ).unwrap();
        store
            .save_terminal_interaction("run", stamp, Some(stamp), true, None)
            .unwrap();
        assert!(store.load_attention().contains_key("run"));
        store
            .writer_for_test()
            .execute("DELETE FROM implementations WHERE id = 'run'", [])
            .unwrap();
        store
            .writer_for_test()
            .execute("DELETE FROM attention WHERE entity_id = 'run'", [])
            .unwrap();
        store
            .save_terminal_interaction("run", stamp, Some(stamp), true, None)
            .unwrap();
        assert!(!store.load_attention().contains_key("run"));
    }
}
