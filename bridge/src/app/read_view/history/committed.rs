//! Separately committed historical row edits share the same immutable root as
//! their existing digest rows and bounded push tails.
use super::{insert, patch_thread_digests, SharedHistory};
use crate::app::read_view::SnapshotDelta;
use crate::app::AppState;
use crate::store::CommittedThreadItems;
use crate::thread::Thread;
use serde_json::{json, Value};
use std::sync::Arc;

#[cfg(test)]
mod tests;

impl SharedHistory {
    pub fn with_committed_items(&self, delta: &CommittedThreadItems) -> Option<Self> {
        if self.metadata["agent"]["id"].as_str() != Some(delta.conversation_id.as_str())
            || self.thread_id != delta.thread_id
            || self.generation != delta.generation_revision
        {
            return None;
        }
        let mut next = self.clone();
        for item in &delta.items {
            if next
                .item(item.sequence())
                .is_none_or(|previous| previous.latest_sequence() < item.latest_sequence())
            {
                next.root = Some(insert(&next.root, Arc::new(item.clone())));
            }
        }
        next.last_sequence = next.last_sequence.max(delta.sequence);
        next.publication_sequence = next.publication_sequence.max(delta.sequence);
        next.metadata["thread_last_sequence"] = json!(next.last_sequence);
        Some(next)
    }

    fn matches(&self, thread: &Thread) -> bool {
        self.thread_id == thread.id && self.generation == thread.generation_revision
    }
}

impl AppState {
    /// Called synchronously under the existing app guard after the Store row
    /// transaction commits, before the ordinary roster finisher can fail.
    pub(in crate::app) fn apply_committed_thread_items(
        &mut self,
        committed: &CommittedThreadItems,
    ) {
        #[cfg(test)]
        let _phase = crate::timing::phase_samples::phase("publication.committed");
        self.committed_entities.apply_thread_items(committed);
        for agents in self
            .plans
            .values_mut()
            .map(|active| &mut active.agents)
            .chain(self.runs.values_mut().map(|active| &mut active.agents))
        {
            for agent in agents.iter_mut() {
                committed.apply_to(&mut agent.thread);
            }
        }
        self.read_view.publish_current(|previous, version| {
            let history = previous
                .history(&committed.conversation_id)?
                .with_committed_items(committed)?;
            let mut facts = previous.facts.as_ref().clone();
            facts.apply_committed_history_delta(committed);
            let mut delta = SnapshotDelta::new(version)
                .put_history(&committed.conversation_id, history)
                .set_facts(Arc::new(facts));
            #[cfg(test)]
            let _phase = crate::timing::phase_samples::phase("publication.committed.chunks");
            for key in previous.keys() {
                let mut row = previous.get(key.method, &key.owner)?.clone();
                #[cfg(test)]
                tests::record_chunk_copy();
                if patch_thread_digests(&mut row, committed) {
                    delta = delta.put(key.method, &key.owner, row);
                }
            }
            Some(delta)
        });
    }

    pub(in crate::app) fn snapshot_thread_sequence(&self, thread: &Thread) -> u64 {
        self.read_view
            .load()
            .history(&thread.agent.id)
            .filter(|history| history.matches(thread))
            .map_or(thread.last_sequence(), |history| {
                history.last_sequence.max(thread.last_sequence())
            })
    }

    pub(in crate::app) fn snapshot_thread_digest(&self, thread: &Thread) -> Value {
        let mut digest = thread.digest_value();
        digest["last_sequence"] = json!(self.snapshot_thread_sequence(thread));
        digest
    }
}
