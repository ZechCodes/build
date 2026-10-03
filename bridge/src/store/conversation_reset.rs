use super::{Store, StoreError};
use crate::agent::Agent;
use crate::attention::Attention;
use crate::thread::ThreadItem;
use std::collections::{HashMap, HashSet};

impl Store {
    pub fn register_conversation_attachment(
        &self,
        conversation_id: &str,
        thread_id: &str,
        path: &str,
    ) -> Result<(), StoreError> {
        self.in_transaction(|tx| {
            tx.execute("INSERT OR IGNORE INTO conversation_attachment_uploads (conversation_id, thread_id, path) VALUES (?1, ?2, ?3)",
                rusqlite::params![conversation_id, thread_id, path])?;
            Ok(())
        })
    }

    /// The reset's complete durable change. No entity/tracker/task identity
    /// is deleted, and a failed transaction leaves the old generation intact.
    pub fn reset_conversation(
        &self,
        conversation_id: &str,
        agents: &[Agent],
        attention: &HashMap<String, Attention>,
        summary_owners: &HashSet<String>,
    ) -> Result<(), StoreError> {
        self.in_transaction(|tx| {
            tx.execute("DELETE FROM thread_items WHERE agent_id = ?1", [conversation_id])?;
            tx.execute("DELETE FROM operations WHERE conversation_id = ?1", [conversation_id])?;
            tx.execute("DELETE FROM conversation_attachment_uploads WHERE conversation_id = ?1", [conversation_id])?;
            for agent in agents {
                tx.execute("DELETE FROM thread_items WHERE agent_id = ?1", [&agent.id])?;
                tx.execute("DELETE FROM operations WHERE conversation_id = ?1", [&agent.id])?;
                tx.execute("DELETE FROM conversation_attachment_uploads WHERE conversation_id = ?1", [&agent.id])?;
                tx.execute("DELETE FROM agent_migration_backups WHERE agent_id = ?1", [&agent.id])?;
                let mut skeleton = agent.clone();
                skeleton.thread.items.clear();
                tx.execute("UPDATE agents SET record = ?2 WHERE id = ?1",
                    rusqlite::params![agent.id, serde_json::to_string(&skeleton).expect("agent serializes")])?;
            }
            for owner in summary_owners {
                tx.execute("UPDATE implementations SET record = json_set(record, '$.last_summary', NULL, '$.last_error', NULL) WHERE id = ?1", [owner])?;
                tx.execute("UPDATE tasks SET record = json_set(record, '$.last_summary', NULL, '$.last_error', NULL) WHERE id = ?1", [owner])?;
            }
            for (owner, record) in attention {
                tx.execute("INSERT INTO attention (entity_id, record) VALUES (?1, ?2) ON CONFLICT(entity_id) DO UPDATE SET record = ?2",
                    rusqlite::params![owner, serde_json::to_string(record).expect("attention serializes")])?;
            }
            Ok(())
        })
    }

    fn reset_conversation_ids(&self, canonical: &str) -> Result<HashSet<String>, StoreError> {
        let conn = self.connection();
        let mut statement = conn.prepare(
            "SELECT id FROM agents WHERE id = ?1 OR json_extract(record, '$.conversation_id') = ?1",
        )?;
        let rows = statement.query_map([canonical], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<HashSet<_>, _>>()
            .map_err(StoreError::from)
    }

    fn reset_message_attachment_leaves(
        &self,
        erased: &HashSet<String>,
    ) -> Result<HashSet<String>, StoreError> {
        let mut removed = HashSet::new();
        for id in erased {
            for item in self.thread_items(id)? {
                if let ThreadItem::Message(message) = item {
                    removed.extend(
                        message
                            .attachments
                            .iter()
                            .filter_map(|attachment| leaf(&attachment.path)),
                    );
                }
            }
        }
        Ok(removed)
    }

    /// Files the erased conversation owns, minus every surviving conversation
    /// or task reference. Read before staging files or changing the database.
    pub fn reset_attachment_leaves(
        &self,
        conversation_id: &str,
    ) -> Result<HashSet<String>, StoreError> {
        let erased = self.reset_conversation_ids(conversation_id)?;
        let mut removed = self.reset_message_attachment_leaves(&erased)?;
        let conn = self.connection();
        let mut uploads =
            conn.prepare("SELECT conversation_id, path FROM conversation_attachment_uploads")?;
        let rows = uploads.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut retained = HashSet::new();
        for row in rows {
            let (owner, path) = row?;
            if let Some(name) = leaf(&path) {
                if erased.contains(&owner) {
                    removed.insert(name);
                } else {
                    retained.insert(name);
                }
            }
        }
        let sql = "SELECT json_extract(a.value, '$.path') FROM thread_items t, json_each(t.item, '$.data.attachments') a WHERE t.agent_id NOT IN (SELECT id FROM agents WHERE id = ?1 OR json_extract(record, '$.conversation_id') = ?1)
            UNION ALL SELECT json_extract(a.value, '$.path') FROM tracker_tasks t, json_each(t.record, '$.attachments') a
            UNION ALL SELECT json_extract(a.value, '$.path') FROM tracker_comments t, json_each(t.record, '$.attachments') a
            UNION ALL SELECT json_extract(a.value, '$.path') FROM operations o, json_each(o.delivery, '$.payload.messages') m, json_each(m.value, '$.attachments') a WHERE o.conversation_id NOT IN (SELECT id FROM agents WHERE id = ?1 OR json_extract(record, '$.conversation_id') = ?1)";
        let mut paths = conn.prepare(sql)?;
        for row in paths.query_map([conversation_id], |row| row.get::<_, String>(0))? {
            if let Some(name) = leaf(&row?) {
                retained.insert(name);
            }
        }
        removed.retain(|name| !retained.contains(name));
        Ok(removed)
    }
}

fn leaf(path: &str) -> Option<String> {
    std::path::Path::new(path)
        .file_name()?
        .to_str()
        .map(str::to_string)
}
