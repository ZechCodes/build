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
        let mut snapshot = {
            #[cfg(test)]
            let _phase = crate::timing::phase_samples::phase("publication.push_facts.clone");
            PushFacts {
                states: previous.facts.states.clone(),
                threads: previous.facts.threads.clone(),
                lists,
                ..PushFacts::default()
            }
        };
        // External checkout state can reflect a changed agent on the same
        // root. Rebuild these cheap rows with the fresh checkout subjects.
        {
            #[cfg(test)]
            let _phase = crate::timing::phase_samples::phase("publication.push_facts.prune_states");
            snapshot
                .states
                .retain(|owner, _| self.plans.contains_key(owner) || self.runs.contains_key(owner));
        }
        #[cfg(test)]
        let owners_phase = crate::timing::phase_samples::phase("publication.push_facts.owners");
        for owner in owners {
            snapshot.states.remove(owner);
            snapshot.threads.remove(owner);
            if self.plans.contains_key(owner) || self.runs.contains_key(owner) {
                {
                    #[cfg(test)]
                    let _source = self.runs.contains_key(owner).then(|| {
                        crate::app::snapshot_projection::work_counts::named_branch::source(
                            owner,
                            crate::app::snapshot_projection::work_counts::named_branch::Source::RunState,
                        )
                    });
                    if let Some(state) = self.entity_state_shared_item(owner) {
                        snapshot.states.insert(owner.clone(), state);
                    }
                }
                {
                    snapshot.threads.insert(
                        owner.clone(),
                        self.push_thread_tails(owner, &previous.facts),
                    );
                }
            }
        }
        #[cfg(test)]
        drop(owners_phase);
        self.refresh_push_thread_sessions(&mut snapshot);
        // Checkout facts carry their own identity and observation fences. Their
        // capture retains those generations while filling affected alias states.
        self.capture_push_checkouts(&mut snapshot, &previous.facts);
        self.capture_push_entities(&mut snapshot, &previous.facts);
        snapshot
    }

    fn refresh_push_thread_sessions(&self, snapshot: &mut PushFacts) {
        #[cfg(test)]
        let _phase = crate::timing::phase_samples::phase("publication.push_facts.sessions");
        // A message can advance the shared project summary for otherwise
        // unchanged conversations. Refresh this small tip metadata while
        // retaining their tail bodies, receipt patches and history floors.
        for (owner, threads) in &mut snapshot.threads {
            for thread in threads {
                thread.tip.project_session = thread
                    .tip
                    .project_id
                    .as_deref()
                    .map(|id| self.session_summary(id));
                thread.tip.workspace_session = thread
                    .tip
                    .workspace_id
                    .as_ref()
                    .map(|_| self.session_summary(owner));
            }
        }
    }
}
