//! Final-only complete native evidence, including bootstrap and service roots.
//! No progress polling and no assumption that a producer tick is a service step.
use super::super::trial_complete::ArtifactSink;
use super::*;
use crate::timing::write_wait::{CompletedInterval, WriteSnapshot};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

pub(super) fn persist(
    prepared: &PreparedBackend,
    rows: &[WriteSnapshot],
    intervals: &[CompletedInterval],
    sink: &dyn ArtifactSink,
) -> Result<(), String> {
    let anchor = Instant::now();
    let registry = &prepared
        .fixture
        .as_ref()
        .ok_or("native final archive lost fixture")?
        .registry;
    let mut counts = BTreeMap::<String, usize>::new();
    for row in rows {
        *counts
            .entry(format!("{:?}/{}", row.cohort, row.method))
            .or_default() += 1;
    }
    let overhead = registry.collector_overhead();
    sink.registry_archive(prepared.name, &json!({
        "schema":"build.native-registry-final.v1", "qualification":false,
        "clock":"signed microseconds relative to this final export's single Instant anchor",
        "scope":"all native rows at pre-cleanup required drain, including prior bootstrap, four explicit producer ticks, autonomous service work and optional notices",
        "planned_client_originals":prepared.plan.iter().map(|slot| &slot.original_id).collect::<Vec<_>>(),
        "cohort_method_counts":counts,
        "rows":rows.iter().map(|row| native_row(row, anchor)).collect::<Vec<_>>(),
        "completed_intervals":intervals.iter().map(|row| native_interval(row, anchor)).collect::<Vec<_>>(),
        "collector_overhead":{
            "writer_acquisitions":overhead.writer_acquisitions,
            "writer_lock_wait_us":us(overhead.writer_lock_wait),"writer_mutation_hold_us":us(overhead.writer_mutation_hold),
            "max_writer_lock_wait_us":us(overhead.max_writer_lock_wait),"max_writer_mutation_hold_us":us(overhead.max_writer_mutation_hold)
        }
    }))
}
fn native_row(row: &WriteSnapshot, anchor: Instant) -> Value {
    json!({
        "write_id":format!("{:?}",row.id),"original_id":row.original_id,"method":row.method,"cohort":format!("{:?}",row.cohort),
        "intake_relative_us":at(row.intake,anchor),"admitted_relative_us":row.admitted_at.map(|value| at(value,anchor)),
        "acknowledged_relative_us":row.acknowledged_at.map(|value| at(value,anchor)),"completed_relative_us":row.completed_at.map(|value| at(value,anchor)),
        "root_finished":row.root_finished,"outstanding":row.outstanding.iter().map(|id|format!("{id:?}")).collect::<Vec<_>>(),
        "active":row.active.iter().map(|interval| json!({"id":format!("{:?}",interval.id),"obligation":format!("{:?}",interval.obligation),"category":format!("{:?}",interval.category),"started_relative_us":at(interval.started,anchor)})).collect::<Vec<_>>(),
        "known_wait_union_us":us(row.known_wait_union),"sql_service_union_us":us(row.sql_service_union),"blocked_or_sql_union_us":us(row.blocked_or_sql_union),
        "conservative_upper_us":us(row.conservative_upper),"own_external_union_us":us(row.own_external_union),"own_external_active_us":us(row.own_external_active),
        "max_chargeable_interval_us":us(row.max_chargeable_interval),"interval_breached":row.interval_breached,"cumulative_breached":row.cumulative_breached,
        "cancelled":row.cancelled,"abandoned":row.abandoned
    })
}
fn native_interval(row: &CompletedInterval, anchor: Instant) -> Value {
    json!({"id":format!("{:?}",row.id),"origins":row.origins.iter().map(|id|format!("{id:?}")).collect::<Vec<_>>(),
        "obligation":format!("{:?}",row.obligation),"category":format!("{:?}",row.category),
        "started_relative_us":at(row.started,anchor),"ended_relative_us":at(row.ended,anchor)})
}
fn at(value: Instant, anchor: Instant) -> i64 {
    if value >= anchor {
        us(value.duration_since(anchor)) as i64
    } else {
        -(us(anchor.duration_since(value)) as i64)
    }
}
fn us(value: Duration) -> u64 {
    value.as_micros().min(i64::MAX as u128) as u64
}
