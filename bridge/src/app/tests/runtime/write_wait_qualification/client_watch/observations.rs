//! Archived snapshots, cohort rates, and final lifecycle reconciliation.
use super::*;
use std::sync::atomic::Ordering;

impl ClientWatch {
    /// Final boundary only: inspect terminal receipts without requiring a
    /// relative schedule to have started or treating cancellation as a reply.
    pub fn unfinished_final_originals(&self) -> Vec<String> {
        self.inner
            .registry
            .lock()
            .unwrap()
            .rows
            .iter()
            .filter_map(|(id, evidence)| {
                let row = evidence.row.lock().unwrap();
                (row.final_at.is_none() || row.final_outcome.is_none() || row.finished_at.is_none())
                    .then(|| id.clone())
            })
            .collect()
    }

    /// Full archive, including future, unoffered, unsent, completed and cancelled
    /// originals. This deliberately scans all evidence only when requested.
    pub fn snapshot(&self) -> Vec<ClientRow> {
        self.snapshot_report().rows
    }

    /// Every active age in this report is capped with the same sampled instant.
    /// Per-event timestamps remain actual observations; never reinterpret this
    /// report's pending rows using a later Instant::now().
    pub fn snapshot_report(&self) -> ClientSnapshot {
        let now = (self.inner.clock)();
        let rows: Vec<_> = self
            .inner
            .registry
            .lock()
            .unwrap()
            .rows
            .values()
            .cloned()
            .collect();
        let rows = rows
            .into_iter()
            .map(|evidence| {
                let stored = evidence.row.lock().unwrap();
                let mut row = ClientRow {
                    original_id: evidence.deadline.original_id.clone(),
                    cohort: evidence.deadline.cohort,
                    intended_at: evidence
                        .deadline
                        .schedule
                        .intended_at()
                        .expect("snapshot relative evidence after T0"),
                    registered_at: stored.registered_at,
                    implicit_schedule: stored.implicit_schedule,
                    offered_at: stored.offered_at,
                    sent_at: stored.sent_at,
                    server_registered_at: stored.server_registered_at,
                    admitted_at: stored.admitted_at,
                    final_at: stored.final_at,
                    final_outcome: stored.final_outcome,
                    cancelled_at: stored.cancelled_at,
                    finished_at: stored.finished_at,
                    cancelled: stored.cancelled_at.is_some(),
                    breached: false,
                };
                drop(stored);
                if row.age_at(now) > LIMIT {
                    evidence.deadline.breached.store(true, Ordering::Release);
                }
                row.breached = evidence.deadline.breached.load(Ordering::Acquire);
                row
            })
            .collect();
        ClientSnapshot {
            sampled_at: now,
            rows,
        }
    }

    /// A transport demultiplexer must call this for unknown response IDs rather
    /// than silently dropping them or inventing a scheduled/server origin row.
    pub fn record_unmatched_reply(&self, original_id: &str) {
        self.inner
            .unmatched_replies
            .lock()
            .unwrap()
            .push(original_id.into());
    }

    /// Run after joined finite drain. Being below 5s is insufficient: every
    /// scheduled original needs an actual offer/send/final reply. Reads/exempt
    /// traffic do not require or fabricate semantic-write registry origins.
    pub fn reconcile_final(&self) -> FinalReconciliation {
        let snapshot = self.snapshot_report();
        let mut violations = Vec::new();
        for row in &snapshot.rows {
            let missing = row.missing_lifecycle();
            if !missing.is_empty() {
                violations.push(format!("{}: {}", row.original_id, missing.join(", ")));
            }
        }
        for unmatched in self.inner.unmatched_replies.lock().unwrap().iter() {
            violations.push(format!("unmatched reply identity: {unmatched}"));
        }
        FinalReconciliation {
            sampled_at: snapshot.sampled_at,
            rows: snapshot.rows,
            violations,
        }
    }
}

impl ClientRow {
    fn missing_lifecycle(&self) -> Vec<&'static str> {
        let semantic = self.cohort == TrafficCohort::SemanticWrite;
        [
            (self.implicit_schedule, "explicit schedule"),
            (self.offered_at.is_none(), "actual offer"),
            (self.sent_at.is_none(), "actual send"),
            (
                semantic && self.server_registered_at.is_none(),
                "actual server origin",
            ),
            (semantic && self.admitted_at.is_none(), "actual admission"),
            (self.final_at.is_none(), "final reply"),
            (
                self.final_outcome != Some(FinalOutcome::Reply),
                "successful transport reply",
            ),
            (self.cancelled, "uncancelled lifecycle"),
            (self.finished_at.is_none(), "settled lifecycle"),
            (self.breached, "deadline within 5 seconds"),
        ]
        .into_iter()
        .filter_map(|(missing, label)| missing.then_some(label))
        .collect()
    }

    pub fn age_at(&self, now: Instant) -> Duration {
        self.finished_at
            .map(|finished| finished.min(now))
            .unwrap_or(now)
            .saturating_duration_since(self.intended_at)
    }
}
