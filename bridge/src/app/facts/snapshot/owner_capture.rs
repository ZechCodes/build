//! Reuse immutable owner facts when the publication's structural shape agrees
//! with the accepted global projection.
use super::*;

impl AppState {
    /// The caller has checked the complete entity/checkout layout and includes
    /// aliases plus every owner with live session or clock inputs in `owners`.
    /// Registry and lifecycle changes use the full capture instead.
    pub(in crate::app) fn push_facts_incremental_snapshot(
        &self,
        lists: SharedJson,
        owners: &BTreeSet<String>,
    ) -> PushFacts {
        #[cfg(test)]
        let _phase = crate::timing::phase_samples::phase("publication.push_facts");
        let previous = self.read_view.load();
        let mut snapshot = PushFacts {
            states: previous.facts.states.clone(),
            threads: previous.facts.threads.clone(),
            lists,
            ..PushFacts::default()
        };
        // External checkout state can reflect a changed agent on the same
        // root. Rebuild these cheap rows with the fresh checkout subjects.
        snapshot
            .states
            .retain(|owner, _| self.plans.contains_key(owner) || self.runs.contains_key(owner));
        for owner in owners {
            snapshot.states.remove(owner);
            snapshot.threads.remove(owner);
            if self.plans.contains_key(owner) || self.runs.contains_key(owner) {
                if let Some(state) = self.entity_state_item(owner) {
                    snapshot
                        .states
                        .insert(owner.clone(), SharedJson::value(state));
                }
                snapshot.threads.insert(
                    owner.clone(),
                    self.push_thread_tails(owner, &previous.facts),
                );
            }
        }
        // Checkout facts carry their own identity and observation fences. Their
        // capture retains those generations while filling affected alias states.
        self.capture_push_checkouts(&mut snapshot, &previous.facts);
        self.capture_push_entities(&mut snapshot, &previous.facts);
        snapshot
    }
}
