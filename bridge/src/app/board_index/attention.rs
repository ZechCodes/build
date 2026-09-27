use std::collections::HashMap;

use crate::attention::Attention;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::app) struct EntityClock {
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub state_changed_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) struct ConversationWatermark {
    pub previous: Option<u64>,
}

/// Owned persistence payload. It intentionally exposes entries rather than the
/// component's backing map; the store adapter consumes it at the exact current
/// persistence boundary.
pub(in crate::app) struct AttentionPersistence {
    entries: Vec<(String, Attention)>,
}

impl AttentionPersistence {
    pub(in crate::app) fn into_entries(self) -> Vec<(String, Attention)> {
        self.entries
    }
}

pub(in crate::app) struct AttentionIndex {
    entries: HashMap<String, Attention>,
    conversation_attention_sequence: HashMap<String, u64>,
    entity_created_at: HashMap<String, String>,
    entity_updated_at: HashMap<String, String>,
    entity_state_changed_at: HashMap<String, String>,
    entity_last_state: HashMap<String, String>,
}

impl AttentionIndex {
    pub(in crate::app) fn new(entries: HashMap<String, Attention>) -> Self {
        Self {
            entries,
            conversation_attention_sequence: HashMap::new(),
            entity_created_at: HashMap::new(),
            entity_updated_at: HashMap::new(),
            entity_state_changed_at: HashMap::new(),
            entity_last_state: HashMap::new(),
        }
    }

    pub(in crate::app) fn persistence(&self) -> AttentionPersistence {
        AttentionPersistence {
            entries: self
                .entries
                .iter()
                .map(|(id, attention)| (id.clone(), attention.clone()))
                .collect(),
        }
    }

    /// Boot hydration replaces only persisted attention entries. Conversation
    /// watermarks and clocks are separate state and are intentionally untouched.
    pub(in crate::app) fn replace_entries(&mut self, entries: HashMap<String, Attention>) {
        self.entries = entries;
    }

    pub(in crate::app) fn attention(&self, entity_id: &str) -> Option<&Attention> {
        self.entries.get(entity_id)
    }

    #[cfg(test)]
    pub(in crate::app) fn set_legacy_dismissed_through(&mut self, entity_id: &str, line: u64) {
        self.ensure_attention(entity_id).dismissed_through = line;
    }

    #[cfg(test)]
    pub(in crate::app) fn has_agent_cursor(&self, entity_id: &str, agent_id: &str) -> bool {
        self.entries
            .get(entity_id)
            .is_some_and(|attention| attention.agent_read_sequences.contains_key(agent_id))
    }

    #[cfg(test)]
    pub(in crate::app) fn last_worked_at(&self, entity_id: &str) -> Option<&str> {
        self.entries
            .get(entity_id)
            .and_then(|attention| attention.last_worked_at.as_deref())
    }

    pub(in crate::app) fn attention_ids(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    fn ensure_attention(&mut self, entity_id: &str) -> &mut Attention {
        self.entries.entry(entity_id.to_string()).or_default()
    }

    pub(in crate::app) fn clock(&self, entity_id: &str) -> EntityClock {
        EntityClock {
            created_at: self.entity_created_at.get(entity_id).cloned(),
            updated_at: self.entity_updated_at.get(entity_id).cloned(),
            state_changed_at: self.entity_state_changed_at.get(entity_id).cloned(),
        }
    }

    pub(in crate::app) fn record_created_if_absent(&mut self, entity_id: &str, at: String) {
        self.entity_created_at
            .entry(entity_id.to_string())
            .or_insert(at);
    }

    pub(in crate::app) fn record_updated(&mut self, entity_id: &str, at: String) {
        self.entity_updated_at.insert(entity_id.to_string(), at);
    }

    /// Exact recover_plan/recover_run hydration: all four stored/recovered facts
    /// replace any existing values unconditionally.
    pub(in crate::app) fn restore_entity_clocks(
        &mut self,
        entity_id: String,
        created_at: String,
        updated_at: String,
        state_changed_at: String,
        last_state: String,
    ) {
        self.entity_created_at.insert(entity_id.clone(), created_at);
        self.entity_updated_at.insert(entity_id.clone(), updated_at);
        self.entity_state_changed_at
            .insert(entity_id.clone(), state_changed_at);
        self.entity_last_state.insert(entity_id, last_state);
    }

    /// Returns true only when the wire state changed and its clock was advanced.
    pub(in crate::app) fn observe_state(
        &mut self,
        entity_id: &str,
        state: String,
        at: String,
    ) -> bool {
        if self.entity_last_state.get(entity_id) == Some(&state) {
            return false;
        }
        self.entity_state_changed_at
            .insert(entity_id.to_string(), at);
        self.entity_last_state.insert(entity_id.to_string(), state);
        true
    }

    /// Exact clock cleanup used by existing run/task removal call sites. This
    /// deliberately leaves attention and conversation watermarks untouched.
    pub(in crate::app) fn remove_entity_clocks(&mut self, entity_id: &str) {
        self.entity_created_at.remove(entity_id);
        self.entity_updated_at.remove(entity_id);
        self.entity_state_changed_at.remove(entity_id);
        self.entity_last_state.remove(entity_id);
    }

    pub(in crate::app) fn interact(&mut self, entity_id: &str, now: &str) {
        self.ensure_attention(entity_id).interact(now);
    }

    /// Applies one already-validated seen operation. State visibility and every
    /// selected agent cursor move together; the adapter persists exactly once
    /// after this returns.
    pub(in crate::app) fn mark_seen(
        &mut self,
        entity_id: &str,
        state_changed_at: Option<&str>,
        cursors: &[(String, u64)],
    ) {
        let attention = self.ensure_attention(entity_id);
        if let Some(state_changed_at) = state_changed_at {
            attention.see(state_changed_at);
        }
        for (agent_id, sequence) in cursors {
            attention.read_through(agent_id, *sequence);
        }
    }

    /// The adapter supplies whether this is the primary agent, preserving the
    /// legacy entity-wide cursor inheritance without giving this index a roster.
    pub(in crate::app) fn read_cursor(
        &self,
        entity_id: &str,
        agent_id: &str,
        is_primary_agent: bool,
    ) -> u64 {
        let Some(attention) = self.entries.get(entity_id) else {
            return 0;
        };
        let cursor = attention.cursor_for(agent_id);
        if is_primary_agent {
            cursor.max(attention.last_read_sequence)
        } else {
            cursor
        }
    }

    pub(in crate::app) fn set_muted(&mut self, entity_id: &str, muted: bool) {
        self.ensure_attention(entity_id).muted = muted;
    }

    pub(in crate::app) fn set_entity_dismissal(
        &mut self,
        entity_id: &str,
        lines: &[(String, u64)],
    ) {
        let attention = self.ensure_attention(entity_id);
        attention.dismiss_messages();
        for (agent_id, last_attention_sequence) in lines {
            attention.dismiss_agent_through(agent_id, *last_attention_sequence);
        }
    }

    /// Transfer the exact bookkeeping removed with an adopted entity-less row.
    /// The caller preserves the original conditional and persistence boundary.
    pub(in crate::app) fn transfer_adopted_row(
        &mut self,
        entity_id: &str,
        first_observed_at: Option<String>,
        was_dismissed: bool,
    ) {
        let attention = self.ensure_attention(entity_id);
        attention.first_observed_at = first_observed_at;
        if was_dismissed {
            attention.dismiss_messages();
        }
    }

    /// Removes exactly the supplied row keys and returns their records for the
    /// adapter's existing combined dismissal/first-observed aggregation.
    pub(in crate::app) fn take_row_attentions(&mut self, row_keys: &[String]) -> Vec<Attention> {
        row_keys
            .iter()
            .filter_map(|key| self.entries.remove(key))
            .collect()
    }

    pub(in crate::app) fn observe_working(
        &mut self,
        entity_id: &str,
        working: bool,
        now: &str,
    ) -> bool {
        self.ensure_attention(entity_id)
            .observe_working(working, now)
    }

    pub(in crate::app) fn observe_status(
        &mut self,
        entity_id: &str,
        working: bool,
        changed_at: &str,
        last_worked_at: Option<&str>,
    ) -> bool {
        self.ensure_attention(entity_id)
            .observe_status(working, changed_at, last_worked_at)
    }

    pub(in crate::app) fn seed_anchor(&mut self, entity_id: &str, created_at: &str) -> bool {
        let attention = self.ensure_attention(entity_id);
        if attention.anchor_at.is_some() {
            return false;
        }
        attention.seed_anchor(created_at);
        true
    }

    pub(in crate::app) fn note_user_message(&mut self, entity_id: &str, at: &str) {
        self.ensure_attention(entity_id).note_user_message(at);
    }

    /// The adapter resolves the capture and passes its already-owned anchor.
    pub(in crate::app) fn inherit_anchor(&mut self, entity_id: &str, anchor: &str) {
        self.ensure_attention(entity_id)
            .inherit_anchor(anchor, None);
    }

    pub(in crate::app) fn dismiss_row_at_head(&mut self, row_key: &str, head: Option<&str>) {
        self.ensure_attention(row_key).dismiss_at_head(head);
    }

    pub(in crate::app) fn row_is_dismissed(&self, row_key: &str, head: Option<&str>) -> bool {
        self.entries
            .get(row_key)
            .is_some_and(|attention| attention.is_dismissed_at_head(head))
    }

    pub(in crate::app) fn is_muted(&self, entity_id: &str) -> bool {
        self.entries
            .get(entity_id)
            .is_some_and(|attention| attention.muted)
    }

    pub(in crate::app) fn observe_row(&mut self, row_key: &str, now: &str) -> bool {
        self.ensure_attention(row_key).observe(now)
    }

    pub(in crate::app) fn invalidate_dismissal(&mut self, entity_id: &str) -> bool {
        self.entries.get_mut(entity_id).is_some_and(|attention| {
            let was_active = attention.dismissal_active;
            attention.invalidate_dismissal();
            was_active
        })
    }

    pub(in crate::app) fn remove_agent_cursor(
        &mut self,
        entity_id: &str,
        agent_id: &str,
        invalidate_dismissal: bool,
    ) {
        let Some(attention) = self.entries.get_mut(entity_id) else {
            return;
        };
        attention.agent_read_sequences.remove(agent_id);
        if invalidate_dismissal {
            attention.invalidate_dismissal();
        }
    }

    pub(in crate::app) fn seed_anchor_from_history(
        &mut self,
        entity_id: &str,
        created_at: &str,
        user_message_times: &[String],
    ) {
        let attention = self.ensure_attention(entity_id);
        attention.seed_anchor(created_at);
        for at in user_message_times {
            attention.note_user_message(at);
        }
    }

    pub(in crate::app) fn migrate_dismissal_to_messages(
        &mut self,
        entity_id: &str,
        lines: &[(String, u64)],
        was_still_dismissed: bool,
    ) -> bool {
        self.entries.get_mut(entity_id).is_some_and(|attention| {
            attention.migrate_dismissal_to_messages(lines, was_still_dismissed)
        })
    }

    pub(in crate::app) fn close_recovered_working_intervals(&mut self) -> bool {
        let mut changed = false;
        for attention in self.entries.values_mut() {
            changed = attention.close_recovered_working_interval() || changed;
        }
        changed
    }

    #[cfg(test)]
    pub(super) fn seed_legacy_read_cursor(&mut self, entity_id: &str, sequence: u64) {
        self.ensure_attention(entity_id).last_read_sequence = sequence;
    }

    pub(in crate::app) fn seed_conversation(&mut self, conversation_id: String, sequence: u64) {
        self.conversation_attention_sequence
            .insert(conversation_id, sequence);
    }

    pub(in crate::app) fn conversation_watermark(
        &self,
        conversation_id: &str,
    ) -> ConversationWatermark {
        ConversationWatermark {
            previous: self
                .conversation_attention_sequence
                .get(conversation_id)
                .copied(),
        }
    }

    /// Replaces the watermark with the supplied sequence exactly. It is not
    /// forced monotonic because current notification behavior uses insertion.
    /// Returns true on first observation so the adapter can suppress first-look
    /// notification while still advancing the watermark.
    pub(in crate::app) fn advance_conversation(
        &mut self,
        conversation_id: String,
        sequence: u64,
    ) -> bool {
        self.conversation_attention_sequence
            .insert(conversation_id, sequence)
            .is_none()
    }
}
