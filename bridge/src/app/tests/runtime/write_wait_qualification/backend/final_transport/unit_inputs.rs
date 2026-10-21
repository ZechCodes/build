//! Explicit unit fixture, never a native-run receipt.
use super::*;

pub(super) fn unit_inputs() -> (Vec<Slot>, ClientWatch, WatchReport, McpShutdownReport) {
    let base = Instant::now();
    let watch = ClientWatch::new(Arc::new(move || base));
    let plan = vec![
        slot("unit-write", Traffic::Write, Route::AgentMcp, 4),
        slot("unit-read", Traffic::Read, Route::SpaRpc, 0),
        slot("unit-exempt", Traffic::Exempt, Route::SpaRpc, 0),
    ];
    for (id, cohort) in [
        ("unit-write", TrafficCohort::SemanticWrite),
        ("unit-read", TrafficCohort::Read),
        ("unit-exempt", TrafficCohort::Exempt),
    ] {
        watch.register_cohort_at(id, base, cohort);
        watch.offer(id);
        watch.sent(id);
        if cohort == TrafficCohort::SemanticWrite {
            watch.server_registered(id);
            watch.admitted(id);
        }
        watch.complete_with_outcome(id, FinalOutcome::Reply);
    }
    let report = WatchReport {
        failed: false,
        panicked: false,
        polls: 1,
        inspected_deadlines: 1,
        still_indexed: 0,
        breaches: Vec::new(),
    };
    let call = McpCallRow {
        original_id: "unit-write".into(),
        endpoint: 0,
        method: "unit-method".into(),
        read: false,
        queued_at: base,
        socket_wait_started_at: Some(base),
        socket_acquired_at: Some(base),
        sent_at: Some(base),
        final_reply_at: Some(base),
        response_ok: Some(true),
        response_bytes: Some(2),
        response_sha256: Some(format!("{:x}", sha2::Sha256::digest(b"{}"))),
        actual_wire_id: None,
        client_closed_at: Some(base),
        server_joined_at: Some(base),
        worker_finished_at: Some(base),
        worker_reaped_at: Some(base),
        abandoned: false,
        error: None,
    };
    let sockets = (0..8)
        .map(|endpoint| SocketEvidence {
            endpoint,
            agent: format!("unit-agent-{endpoint}"),
            passive_router: false,
            active: 0,
            maximum_active: usize::from(endpoint == 0),
            limit: 2,
        })
        .collect();
    let mcp = McpShutdownReport {
        quiet: true,
        unfinished: Vec::new(),
        evidence: McpEvidence {
            calls: BTreeMap::from([("unit-write".into(), call)]),
            sockets,
            errors: Vec::new(),
            registry_archive: None,
        },
    };
    (plan, watch, report, mcp)
}
fn slot(id: &str, traffic: Traffic, route: Route, caller: usize) -> Slot {
    Slot {
        original_id: id.into(),
        sequence: 0,
        traffic,
        route,
        caller,
        owner: 0,
        due_us: 0,
        class: None,
        finite_target_us: None,
        finite_ceiling_us: None,
        method: Some("unit-method".into()),
    }
}
