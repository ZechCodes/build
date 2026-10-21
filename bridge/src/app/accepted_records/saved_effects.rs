//! Original-specific observers of the existing native save/publication paths.
use super::{AcceptedRecord, PresencePersistence, PresenceRecordWitness, SeenRecordWitness};
use crate::app::read_context::IssuedCodingAddress;
use crate::app::AppState;
use crate::attention::Attention;
use crate::session_summary::UserSession;
use std::collections::{HashMap, HashSet};

/// Stack-local request values; never a second collector or retained journal.
pub(in crate::app) struct SeenProbe {
    originals: Vec<String>,
    address: IssuedCodingAddress,
    target: u64,
    primary: bool,
}

impl AppState {
    pub(in crate::app) fn test_prepare_seen(
        &self,
        owner: &str,
        agent: Option<&str>,
        reached: &[(String, u64)],
    ) -> Option<SeenProbe> {
        self.accepted_records.as_ref()?;
        let context = crate::timing::write_wait::current_context()?;
        let agent = agent?;
        let mut targets = reached.iter().filter(|(id, _)| id == agent);
        let target = targets.next()?.1;
        if targets.next().is_some() {
            return None;
        }
        let address = self.live_mcp_issued_address(owner, agent).ok()?;
        let primary = self.entity_agents(owner).ok()?.is_primary(agent);
        Some(SeenProbe {
            originals: context.original_ids(),
            address,
            target,
            primary,
        })
    }

    /// Only the exact successful Store call supplies this payload and live set.
    /// Failed/no-store calls never reach here; the installed row is memory-only.
    pub(in crate::app) fn test_record_saved_seen(
        &self,
        probe: SeenProbe,
        saved: &HashMap<String, Attention>,
        live: &HashSet<String>,
    ) {
        let Some(records) = &self.accepted_records else {
            return;
        };
        let Some(witness) = self.test_seen_witness(probe, saved, live) else {
            return;
        };
        records.record(AcceptedRecord::Seen(witness));
    }

    fn test_seen_witness(
        &self,
        probe: SeenProbe,
        saved: &HashMap<String, Attention>,
        live: &HashSet<String>,
    ) -> Option<SeenRecordWitness> {
        let address = &probe.address;
        if !live.contains(&address.owner)
            || self
                .live_mcp_issued_address(&address.owner, &address.agent)
                .ok()?
                .ne(address)
        {
            return None;
        }
        let attention = saved.get(&address.owner)?;
        let mut cursor = attention.cursor_for(&address.agent);
        if probe.primary {
            cursor = cursor.max(attention.last_read_sequence);
        }
        let published = self.test_published_seen_cursor(address)?;
        if cursor < probe.target || cursor != published {
            return None;
        }
        Some(SeenRecordWitness {
            original_ids: probe.originals,
            owner: address.owner.clone(),
            agent: address.agent.clone(),
            conversation_owner: address.canonical_owner.clone(),
            conversation_id: address.conversation.clone(),
            thread_id: address.thread_id.clone(),
            generation_revision: address.generation,
            actual_reached_sequence: probe.target,
            saved_through_sequence: cursor,
            published_through_sequence: published,
            native_revision: None,
        })
    }

    fn test_published_seen_cursor(&self, address: &IssuedCodingAddress) -> Option<u64> {
        let view = self.read_view.load();
        let payload = view.get("agent.list", &address.owner)?;
        let mut rows = payload["agents"]
            .as_array()?
            .iter()
            .filter(|row| row["id"] == address.agent);
        let row = rows.next()?;
        if rows.next().is_some()
            || row["conversation_id"] != address.conversation
            || row["thread_id"] != address.thread_id
            || row["thread_generation_revision"].as_u64() != Some(address.generation)
        {
            return None;
        }
        row["read_through_sequence"].as_u64()
    }

    /// Generic user activity updates only this one authoritative scalar after
    /// actual Store success; it emits no Presence record for sibling writes.
    pub(in crate::app) fn test_note_saved_user_session(&self, saved: UserSession) {
        let Some(records) = &self.accepted_records else {
            return;
        };
        let mut known = records.known_durable_user_session.lock().unwrap();
        assert!(
            known.is_some(),
            "actual durable setup must precede native observations"
        );
        *known = Some(Some(saved));
    }

    /// Explicit user.present alone calls this after its native outcome is known.
    pub(in crate::app) fn test_record_presence(&self, persistence: PresencePersistence) {
        let Some(records) = &self.accepted_records else {
            return;
        };
        let Some(context) = crate::timing::write_wait::current_context() else {
            return;
        };
        let Some(published) = self.test_published_user_session() else {
            return;
        };
        let durable_at_execution = records
            .known_durable_user_session
            .lock()
            .unwrap()
            .expect("actual durable setup must precede native observations");
        records.record(AcceptedRecord::Presence(PresenceRecordWitness {
            original_ids: context.original_ids(),
            local: self.user_session,
            published,
            persistence,
            durable_at_execution,
        }));
    }
}
