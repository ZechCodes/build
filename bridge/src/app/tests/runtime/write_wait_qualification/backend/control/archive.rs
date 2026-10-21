//! Actual bounded cursor batches; one final complete registry export only.
use super::*;
use crate::timing::write_wait::archive::{
    CompletedIntervalsCursor, CompletedSnapshotsCursor, MAX_ARCHIVE_BATCH,
};
use crate::timing::write_wait::{WriteCohort, WriteSnapshot, WriteWaitRegistry};
use std::collections::BTreeSet;

pub(super) struct Pump {
    completed: Arc<completed_origins::CompletedOrigins>,
    errors: BTreeSet<String>,
    snapshots: CompletedSnapshotsCursor,
    intervals: CompletedIntervalsCursor,
    imported: BTreeSet<String>,
    intended: BTreeSet<String>,
    stages: BTreeSet<(String, Stage)>,
    actual_interval_rows: usize,
    actual_finite_ids: BTreeSet<crate::timing::write_wait::WaitId>,
    finalized: bool,
}
impl Pump {
    pub fn new(
        registry: &Arc<WriteWaitRegistry>,
        completed: Arc<completed_origins::CompletedOrigins>,
        intended: BTreeSet<String>,
    ) -> Self {
        Self {
            completed,
            errors: BTreeSet::new(),
            snapshots: registry.completed_snapshots_cursor(),
            intervals: registry.completed_intervals_cursor(),
            imported: BTreeSet::new(),
            stages: BTreeSet::new(),
            intended,
            actual_interval_rows: 0,
            actual_finite_ids: BTreeSet::new(),
            finalized: false,
        }
    }
    pub fn tick(
        &mut self,
        registry: &WriteWaitRegistry,
        observe: &Observation,
    ) -> Result<(), String> {
        if let Err(error) = self.batch(registry, observe) {
            self.errors.insert(error);
        }
        self.result()
    }
    fn result(&self) -> Result<(), String> {
        if self.errors.is_empty() {
            Ok(())
        } else {
            Err(self.errors.iter().cloned().collect::<Vec<_>>().join("; "))
        }
    }
    fn batch(
        &mut self,
        registry: &WriteWaitRegistry,
        observe: &Observation,
    ) -> Result<bool, String> {
        if self.finalized {
            return Ok(false);
        }
        // Scaffolds returning empty batches stay empty actual evidence. No
        // snapshot fallback on this progress path; normal600 is still held.
        let rows = registry
            .completed_snapshots_since(&mut self.snapshots, MAX_ARCHIVE_BATCH)
            .map_err(|error| format!("actual registry cursor: {error:?}"))?;
        let row_count = rows.len();
        for row in rows {
            if row.cohort == WriteCohort::SemanticWrite {
                self.imported.insert(row.original_id.clone());
                // Exact immutable identity set; no prefix filtering/classification.
                if self.intended.contains(&row.original_id) {
                    if let Err(error) = self.import_stages(&row, observe) {
                        self.errors.insert(error);
                    }
                }
            }
            // The cursor advanced over this entire batch. Process every row,
            // even after a prior import error, and validate cohort before any
            // proof credit. Publish after stages so waiters see real milestones.
            if let Err(error) = self.completed.import(&row) {
                self.errors.insert(error);
            }
        }
        let intervals = registry
            .completed_intervals_since(&mut self.intervals, MAX_ARCHIVE_BATCH)
            .map_err(|error| format!("actual interval cursor: {error:?}"))?;
        let interval_count = intervals.len();
        self.actual_interval_rows += interval_count;
        for interval in intervals {
            if interval.category == IntervalCategory::FiniteService {
                self.actual_finite_ids.insert(interval.id);
            }
        }
        Ok(row_count > 0 || interval_count > 0)
    }
    fn import_stages(&mut self, row: &WriteSnapshot, observe: &Observation) -> Result<(), String> {
        for (stage, at) in [
            (Stage::ServerIntake, Some(row.intake)),
            (Stage::Admitted, row.admitted_at),
            (Stage::DurableAck, row.acknowledged_at),
        ] {
            if let Some(at) = at {
                if self.stages.insert((row.original_id.clone(), stage)) {
                    if let Err(error) = observe.stage(&row.original_id, stage, at) {
                        self.errors.insert(error);
                    }
                }
            }
        }
        Ok(())
    }
    fn catch_up(
        &mut self,
        registry: &WriteWaitRegistry,
        observe: &Observation,
    ) -> Result<(), String> {
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while self.batch(registry, observe)? {
            if std::time::Instant::now() > until {
                return Err(
                    "bounded final cursor catch-up expired; native final archive retained".into(),
                );
            }
        }
        Ok(())
    }
    pub fn finish(
        &mut self,
        prepared: &mut PreparedBackend,
        registry: &WriteWaitRegistry,
        observe: &Observation,
        sink: &dyn ArtifactSink,
    ) -> Result<(), String> {
        if self.finalized {
            return Ok(());
        }
        let cursor_result = self.catch_up(registry, observe);
        self.finalized = true;
        let rows = registry.snapshot(); // Exactly once at final actual drain.
        let intervals = registry.completed_intervals();
        if let Err(error) =
            super::super::native_archive::persist(prepared, registry, &rows, &intervals, sink)
        {
            self.errors.insert(error);
        }
        let intended = prepared
            .plan
            .iter()
            .filter(|slot| slot.traffic == Traffic::Write)
            .map(|slot| slot.original_id.clone())
            .collect::<BTreeSet<_>>();
        for row in rows
            .iter()
            .filter(|row| intended.contains(&row.original_id))
        {
            if let Err(error) = self.import_stages(row, observe) {
                self.errors.insert(error);
            }
        }
        if let Err(error) = archive_final_rows(&rows, &intended, observe) {
            self.errors.insert(error);
        }
        match prepared.services.as_ref() {
            Some(services) => {
                if let Err(error) = archive_final_services(&rows, &intervals, services, observe) {
                    self.errors.insert(error);
                }
            }
            None => {
                self.errors.insert("actual finite services missing".into());
            }
        }
        if let Err(error) = cursor_result {
            self.errors.insert(error);
        }
        self.errors.extend(self.completed.audit());
        let finite_missing = intervals
            .iter()
            .filter(|interval| interval.category == IntervalCategory::FiniteService)
            .any(|interval| !self.actual_finite_ids.contains(&interval.id));
        if intended.iter().any(|id| !self.imported.contains(id)) || finite_missing {
            self.errors.insert(format!("actual work drained but incremental completion proof RED: {} of{} intended roots observed via real cursor; {} actual interval rows", intended.iter().filter(|id| self.imported.contains(*id)).count(), intended.len(), self.actual_interval_rows));
        }
        self.result()
    }
}

use crate::test_support::finite_service::{FixtureServices, ServiceKind};
use crate::timing::write_wait::IntervalCategory;
fn archive_final_rows(
    snapshots: &[WriteSnapshot],
    intended: &BTreeSet<String>,
    observe: &Observation,
) -> Result<(), String> {
    // Final snapshot includes actual background/optional/unmatched roots;
    // this join requires exactly ONE root for each intended semantic original.
    // A separate full raw-registry writer is still normal-factory work.
    let mut by_original = BTreeMap::new();
    for row in snapshots
        .iter()
        .filter(|row| intended.contains(&row.original_id))
    {
        if row.cohort != WriteCohort::SemanticWrite {
            return Err(format!(
                "intended semantic {} has wrong cohort {:?}",
                row.original_id, row.cohort
            ));
        }
        if by_original.insert(row.original_id.clone(), row).is_some() {
            return Err(format!("duplicate server roots for {}", row.original_id));
        }
    }
    for original in intended {
        let row = by_original
            .get(original)
            .ok_or_else(|| format!("accepted semantic origin missing {original}"))?;
        observe.union(
            original,
            UnionTrace {
                write_ids: vec![format!("{:?}", row.id)],
                sampled_at_us: observe.now_us(),
                known_wait_union_us: us(row.known_wait_union),
                sql_service_union_us: us(row.sql_service_union),
                blocked_or_sql_union_us: us(row.blocked_or_sql_union),
                conservative_upper_us: us(row.conservative_upper),
                own_external_union_us: us(row.own_external_union),
                exclusive_active_external_us: us(row.own_external_active),
                max_chargeable_interval_us: us(row.max_chargeable_interval),
                complete: row.root_finished
                    && row.outstanding.is_empty()
                    && row.active.is_empty()
                    && !row.cancelled
                    && !row.abandoned,
            },
        )?;
    }
    Ok(())
}

fn archive_final_services(
    rows: &[WriteSnapshot],
    intervals: &[crate::timing::write_wait::CompletedInterval],
    services: &FixtureServices,
    observe: &Observation,
) -> Result<(), String> {
    let origins = rows
        .iter()
        .map(|row| (row.id, row.original_id.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut seen = BTreeSet::new();
    for sample in services.samples() {
        if !seen.insert(sample.original_id.clone()) {
            return Err(format!("duplicate finite sample {}", sample.original_id));
        }
        observe.service(service_trace(sample, intervals, &origins, observe)?)?;
    }
    Ok(())
}

fn service_trace(
    sample: crate::test_support::finite_service::ServiceSample,
    intervals: &[crate::timing::write_wait::CompletedInterval],
    origins: &BTreeMap<crate::timing::write_wait::WriteId, String>,
    observe: &Observation,
) -> Result<ServiceTrace, String> {
    let matches = intervals
        .iter()
        .filter(|interval| {
            interval.category == IntervalCategory::FiniteService
                && interval
                    .origins
                    .iter()
                    .any(|id| origins.get(id) == Some(&sample.original_id))
        })
        .collect::<Vec<_>>();
    let [interval] = matches.as_slice() else {
        return Err(format!(
            "{} finite sample has {} real service intervals",
            sample.original_id,
            matches.len()
        ));
    };
    if sample.succeeded && sample.operation_before_padding.is_none() {
        return Err(format!(
            "{} successful service has no actual-operation duration",
            sample.original_id
        ));
    }
    let (kind, ceiling_us) = match sample.kind {
        ServiceKind::Filesystem => ("fenced-fs-current-read-replace", 250_000),
        ServiceKind::Git => ("scoped-git-after-source-exclusion-preflight", 500_000),
    };
    Ok(ServiceTrace {
        original_id: sample.original_id,
        registry_interval_id: format!("{:?}", interval.id),
        registry_origins: interval
            .origins
            .iter()
            .map(|id| format!("{id:?}"))
            .collect(),
        kind: kind.into(),
        target_us: us(sample.target),
        ceiling_us,
        actual_us: us(sample.actual),
        succeeded: sample.succeeded,
        abandoned: sample.abandoned,
        operation_before_padding_us: sample.operation_before_padding.map(us),
        finite_service: Interval {
            begin_us: observe.at_us(interval.started),
            end_us: observe.at_us(interval.ended),
        },
    })
}

fn us(duration: std::time::Duration) -> u64 {
    duration.as_micros().min(u64::MAX as u128) as u64
}

#[cfg(test)]
#[path = "archive/tests.rs"]
mod tests;
