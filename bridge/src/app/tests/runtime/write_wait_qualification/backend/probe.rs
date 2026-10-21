//! OFFSOURCE: one immutable/atomic view of existing collectors only.
//! No AppState, service writer, callback or observation-ledger mutex here.
use super::super::client_watch::ClientWatchObservation;
use super::super::mcp_adapter::McpObservation;
use super::super::producer_adapter::ConcreteProducers;
use super::super::trial_complete::{
    IndependentProbe, OpenRegistryRow, OpenService, ProbeSnapshot, ProducerCounts, Violation,
};
use crate::timing::write_wait::{IntervalCategory, WriteCohort, WriteWaitRegistry};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub(super) struct Probe {
    pub registry: Arc<WriteWaitRegistry>,
    pub clients: Arc<ClientWatchObservation>,
    pub producers: Arc<ConcreteProducers>,
    pub mcp: McpObservation,
    pub finite_ceilings: BTreeMap<String, Duration>,
    pub start: std::sync::OnceLock<Instant>,
}

fn us(duration: Duration) -> u64 {
    duration.as_micros().min(u64::MAX as u128) as u64
}
fn required(cohort: WriteCohort) -> bool {
    !matches!(
        cohort,
        WriteCohort::OptionalNotice | WriteCohort::ExemptRead | WriteCohort::ExemptProtocol
    )
}

impl IndependentProbe for Probe {
    fn snapshot(&self) -> ProbeSnapshot {
        // Both interval endcaps AND elapsed finite-service comparisons use
        // this SAME registry-sampled Instant. No later now/stale scalar pair.
        let (now, active) = self.registry.active_snapshot_with_time();
        let mut output = ProbeSnapshot::default();
        for row in active.into_iter().filter(|row| required(row.cohort)) {
            let active_finite_services = row
                .active
                .iter()
                .filter(|interval| interval.category == IntervalCategory::FiniteService)
                .map(|interval| OpenService {
                    registry_interval_id: format!("{:?}", interval.id),
                    elapsed_us: us(now.saturating_duration_since(interval.started)),
                    // Unknown bounded-service origin is instrumentation failure,
                    // not permission to select a target by execution sequence.
                    ceiling_us: self
                        .finite_ceilings
                        .get(&row.original_id)
                        .map_or(0, |ceiling| us(*ceiling)),
                })
                .collect();
            output.registry.push(OpenRegistryRow {
                original_id: row.original_id.clone(),
                write_id: format!("{:?}", row.id),
                known_wait_union_us: us(row.known_wait_union),
                wait_plus_sql_union_us: us(row.blocked_or_sql_union),
                conservative_upper_us: us(row.conservative_upper),
                active_finite_services,
            });
            if !row.outstanding.is_empty() {
                output.deferred_active.push(format!(
                    "{} outstanding={:?}",
                    row.original_id, row.outstanding
                ));
            }
        }
        for producer in self.producers.snapshots() {
            output.producer_counts.insert(
                producer.name.into(),
                ProducerCounts {
                    intended: producer.intended,
                    started: producer.started,
                    completed: producer.completed,
                    failed: producer.failed,
                },
            );
            if let Some(ordinal) = producer.active_ordinal {
                output
                    .producer_active
                    .push(format!("{}/{ordinal}", producer.name));
            }
            if let Some(error) = producer.first_error {
                output.producer_failures.push(error);
            }
            if producer.stop_requested && producer.missed_at_stop > 0 {
                output.producer_failures.push(format!(
                    "{} missed {} intended callbacks",
                    producer.name, producer.missed_at_stop
                ));
            }
        }
        // ClientWatch indexed includes future/unoffered intents: it is NOT
        // accepted receiver work and must never prevent actual cleanup drain.
        // Actual MCP workers retain their ID through owned server join; fresh
        // RPC request futures are joined by the coordinator before its drain.
        output
            .receiver_pending
            .extend(self.mcp.pending().keys().cloned());
        if self.mcp.failed() {
            output
                .producer_failures
                .push("actual MCP transport failed; retained evidence required".into());
        }
        // The minimal observer exposes only existing atomic state. A live
        // sticky failure stops offers now; exact IDs/timestamps remain in the
        // original guard.finish() WatchReport retained by WatchOwner. Neither
        // this probe nor the accessor drains that receiver or recomputes ages.
        if self.clients.failed() {
            let observed = Instant::now();
            output.caller_breaches.push(Violation {
                original_id: None,
                kind: "independent_client_watch_failed".into(),
                observed_at_us: self.start.get().map_or(0, |start| us(observed.saturating_duration_since(*start))),
                measured_us: None,
                limit_us: Some(5_000_000),
                detail: format!("existing client watchdog latched failure; polls={}, indexed={} (includes future schedule); exact per-ID evidence retained in joined WatchReport", self.clients.polls(), self.clients.indexed()),
            });
        }
        output
    }
}
