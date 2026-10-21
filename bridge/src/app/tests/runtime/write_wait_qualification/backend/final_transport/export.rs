//! Actual observed fields only; no inferred send/final/server stage or wire ID.
use super::super::super::client_watch::ClientRow;
use super::*;

fn at(value: Instant, anchor: Instant) -> i64 {
    let (negative, elapsed) = if value >= anchor {
        (false, value.duration_since(anchor))
    } else {
        (true, anchor.duration_since(value))
    };
    let micros = i64::try_from(elapsed.as_micros())
        .expect("actual test duration fits signed i64 microseconds");
    if negative {
        -micros
    } else {
        micros
    }
}

pub(super) fn reconciliation(value: &FinalReconciliation, anchor: Instant) -> Value {
    json!({"sampled_at_us":at(value.sampled_at,anchor),"violations":value.violations,
        "rows":value.rows.iter().map(|row| client_row(row,anchor)).collect::<Vec<_>>()})
}
fn client_row(row: &ClientRow, anchor: Instant) -> Value {
    json!({"original_id":row.original_id,"cohort":format!("{:?}",row.cohort),
        "intended_at_us":at(row.intended_at,anchor),"registered_at_us":at(row.registered_at,anchor),
        "implicit_schedule":row.implicit_schedule,"offered_at_us":row.offered_at.map(|time|at(time,anchor)),
        "sent_at_us":row.sent_at.map(|time|at(time,anchor)),
        "server_registered_at_us":row.server_registered_at.map(|time|at(time,anchor)),
        "admitted_at_us":row.admitted_at.map(|time|at(time,anchor)),
        "final_at_us":row.final_at.map(|time|at(time,anchor)),
        "final_outcome":row.final_outcome.map(|outcome|format!("{outcome:?}")),
        "cancelled_at_us":row.cancelled_at.map(|time|at(time,anchor)),
        "finished_at_us":row.finished_at.map(|time|at(time,anchor)),
        "cancelled":row.cancelled,"breached":row.breached})
}
pub(super) fn watch_report(value: &WatchReport, anchor: Instant) -> Value {
    json!({"failed":value.failed,"panicked":value.panicked,"polls":value.polls,
        "inspected_deadlines":value.inspected_deadlines,"still_indexed":value.still_indexed,
        "breaches":value.breaches.iter().map(|breach|json!({"original_id":breach.original_id,
            "cohort":format!("{:?}",breach.cohort),"intended_at_us":at(breach.intended_at,anchor),
            "observed_at_us":at(breach.observed_at,anchor),"settled_at_us":breach.settled_at.map(|time|at(time,anchor))})).collect::<Vec<_>>()})
}
pub(super) fn mcp_report(value: &McpShutdownReport, anchor: Instant) -> Value {
    let evidence = &value.evidence;
    json!({"quiet":value.quiet,"unfinished":value.unfinished,"errors":evidence.errors,
        "calls":evidence.calls.iter().map(|(key,row)|json!({"archive_key":key,
            "original_id":row.original_id,"endpoint":row.endpoint,"method":row.method,"read":row.read,
            "queued_at_us":at(row.queued_at,anchor),
            "socket_wait_started_at_us":row.socket_wait_started_at.map(|time|at(time,anchor)),
            "socket_acquired_at_us":row.socket_acquired_at.map(|time|at(time,anchor)),
            "sent_at_us":row.sent_at.map(|time|at(time,anchor)),
            "final_reply_at_us":row.final_reply_at.map(|time|at(time,anchor)),
            "response_ok":row.response_ok,"response_bytes":row.response_bytes,"response_sha256":row.response_sha256,
            "actual_wire_id":row.actual_wire_id,
            "client_closed_at_us":row.client_closed_at.map(|time|at(time,anchor)),
            "server_joined_at_us":row.server_joined_at.map(|time|at(time,anchor)),
            "worker_finished_at_us":row.worker_finished_at.map(|time|at(time,anchor)),
            "worker_reaped_at_us":row.worker_reaped_at.map(|time|at(time,anchor)),
            "abandoned":row.abandoned,"error":row.error})).collect::<Vec<_>>(),
        "sockets":evidence.sockets.iter().map(|socket|json!({"endpoint":socket.endpoint,"agent":socket.agent,
            "passive_router":socket.passive_router,"active":socket.active,"maximum_active":socket.maximum_active,"limit":socket.limit})).collect::<Vec<_>>(),
        "registry_archive":evidence.registry_archive.as_ref().map(|archive|json!({
            "collected_at_us":at(archive.collected_at,anchor),"actual_row_count":archive.rows.len(),
            "unimported":archive.unimported.iter().map(|row|json!({"write_id":format!("{:?}",row.row.id),
                "original_id":row.row.original_id,"reason":row.reason})).collect::<Vec<_>>()}))})
}
