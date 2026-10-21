//! OFFSOURCE: final-only exact registry/service archives, outside watchdogs.
//! These functions are NOT the polled archive_into path; concrete backend uses
//! completion-ID cursors and invokes final validation once after true drain.
use super::super::trial_complete::{FrameTrace, Interval, Observation, ServiceTrace, UnionTrace};
use crate::test_support::finite_service::{FixtureServices, ServiceKind};
use crate::timing::completed_frames::CompletedFrame;
use crate::timing::write_wait::{IntervalCategory, WriteCohort, WriteWaitRegistry};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub(super) struct FrameArchive {
    rows: Mutex<Vec<(Instant, CompletedFrame)>>,
}
impl FrameArchive {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            rows: Mutex::new(Vec::new()),
        })
    }
    pub fn receive(&self, frame: CompletedFrame) {
        self.rows.lock().unwrap().push((Instant::now(), frame));
    }
    pub fn drain_into(&self, observe: &Observation) -> Result<(), String> {
        let rows = std::mem::take(&mut *self.rows.lock().unwrap());
        for (sampled, frame) in rows {
            observe.frame(FrameTrace {
                observer_id: 0,
                original_ids: frame.original_ids,
                protocol: format!("{:?}", frame.protocol),
                method: frame.method,
                sampled_at_us: observe.at_us(sampled),
                queued_us: us(frame.queued),
                permit_wait_us: frame.permit_wait.map(us),
                pool_wait_us: frame.pool_wait.map(us),
                lock_wait_us: us(frame.lock_wait),
                held_us: us(frame.held),
                total_us: us(frame.total),
            })?;
        }
        Ok(())
    }
}

pub(super) fn archive_registry(
    registry: &WriteWaitRegistry,
    intended: &BTreeSet<String>,
    observe: &Observation,
) -> Result<(), String> {
    // At final drain only: all real WriteIds are retained in the raw registry
    // artifact, including background/optional/unmatched. Original semantic
    // attribution below requires exactly ONE root for an intended original.
    let snapshots = registry.snapshot();
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

pub(super) fn archive_services(
    registry: &WriteWaitRegistry,
    services: &FixtureServices,
    observe: &Observation,
) -> Result<(), String> {
    let origins = registry
        .snapshot()
        .into_iter()
        .map(|row| (row.id, row.original_id))
        .collect::<BTreeMap<_, _>>();
    let intervals = registry.completed_intervals();
    let mut seen = BTreeSet::new();
    for sample in services.samples() {
        if !seen.insert(sample.original_id.clone()) {
            return Err(format!("duplicate finite sample {}", sample.original_id));
        }
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
        observe.service(ServiceTrace {
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
        })?;
    }
    Ok(())
}

fn us(duration: Duration) -> u64 {
    duration.as_micros().min(u64::MAX as u128) as u64
}
