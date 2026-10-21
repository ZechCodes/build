//! Final evidence/report construction and ownership of unfinished cleanup.
use super::*;
impl RunState<'_> {
    pub(super) fn finish(mut self) -> Result<TrialRun, String> {
        self.producers.stop();
        if let Some(watchdog) = self.watchdog.take() {
            watchdog.finish();
        }
        self.backend
            .archive_into(&self.observe)
            .unwrap_or_else(|error| self.observe.fail(None, "final_archive", error));
        let last_probe = self.probe.snapshot();
        let cleanup_drained = self.obligations_quiet(&last_probe);
        let rows = self
            .observe
            .ledger
            .lock()
            .unwrap()
            .rows
            .values()
            .cloned()
            .collect::<Vec<_>>();
        validate_final_rows(&rows, &self.observe);
        validate_loaded_ready_reads(&rows, &self.observe);
        let final_counts = counts(rows.iter(), None);
        let complete_original_plan = complete_plan(self.at_600.is_some(), &final_counts);
        let ledger = self.observe.ledger.lock().unwrap();
        let mut report = TrialReport {
            schema: "build.qualification.trial.v1-draft".into(),
            trial: self.name,
            provenance: self.provenance,
            started_unix_us: self.started_unix_us,
            offer_window_us: SECONDS * 1_000_000,
            observed_end_us: self.observe.now_us(),
            counts_at_600s: self.at_600,
            final_counts,
            endpoint_observations: endpoint_counts(&rows),
            methods: method_metrics(&rows),
            violations: ledger.violations.clone(),
            failures: self.failures,
            last_probe,
            cleanup_drained,
            backend_closed: self.backend.closed(),
            qualified: ledger.violations.is_empty()
                && complete_original_plan
                && cleanup_drained
                && self.backend.closed()
                && self.infrastructure.is_none(),
            invalid_infrastructure: self.infrastructure,
            coordinator_sql_review_required: sql_only_budget_review_required(
                &rows,
                &ledger.violations,
            ),
            row_file: format!("{}/rows.jsonl", self.name.label()),
            other_frame_count: ledger.other_frames.len(),
            other_frames_file: format!("{}/other-frames.jsonl", self.name.label()),
        };
        let other_frames = ledger.other_frames.clone();
        drop(ledger);
        if let Err(error) = self.sink.finish(&report, &rows, &other_frames) {
            report.qualified = false;
            report.invalid_infrastructure =
                Some(format!("final artifact persistence failed: {error}"));
        }
        let unfinished = (!cleanup_drained || !self.backend.closed()).then(|| UnfinishedCleanup {
            original_ids: self.running.keys().cloned().collect(),
            tasks: self
                .running
                .into_values()
                .map(|(_, _, task)| task)
                .collect(),
            backend: self.backend,
            producers: self.producers,
        });
        Ok(TrialRun { report, unfinished })
    }
}
pub(super) fn complete_plan(reached_end: bool, counts: &BTreeMap<Traffic, TrafficCounts>) -> bool {
    // Final475ms service may finish after600s without shortening its target.
    reached_end
        && [Traffic::Write, Traffic::Read, Traffic::Exempt]
            .iter()
            .all(|traffic| {
                let c = &counts[traffic];
                c.offered == c.intended
                    && c.sent == c.intended
                    && c.verified_success == c.intended
                    && c.failed == 0
            })
}
pub(super) fn endpoint_counts(rows: &[Row]) -> BTreeMap<Stage, usize> {
    let mut result = BTreeMap::new();
    for row in rows {
        for stage in row.milestones_us.keys() {
            *result.entry(*stage).or_insert(0) += 1;
        }
    }
    result
}
