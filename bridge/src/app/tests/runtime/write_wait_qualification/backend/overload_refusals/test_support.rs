//! Actual registry/watch/collector boundary controls. The explicit busy literal
//! tests protocol classification; this is not a live carrier Full/400s result.
use super::super::super::client_watch::{ClientWatch, FinalOutcome, TrafficCohort, WatchReport};
use super::super::super::trial_complete::{Envelope, Stage, TrialName, UnionTrace};
use super::*;
use crate::timing::write_wait::WriteWaitRegistry;
use serde_json::json;
use std::time::Duration;

pub(in super::super) struct Bundle {
    pub token: Arc<VerifiedRefusals>,
    pub plan: Vec<Slot>,
    pub watch: ClientWatch,
    pub watch_report: WatchReport,
}
pub(super) struct NativeFixture {
    pub audit: Arc<RefusalAudit>,
    pub slot: Slot,
    pub registry: Arc<WriteWaitRegistry>,
    pub observe: Observation,
    pub accepted: AcceptedRefusals,
    pub watch: ClientWatch,
    pub watch_report: WatchReport,
    pub candidate: ControlRefusal,
    pub raw: ReplyProof,
    pub intended: Instant,
}
/// Caller owns isolated_git_test! entry. No private token fields constructed.
pub(super) fn native_fixture() -> NativeFixture {
    native_fixture_with(false, false)
}
pub(super) fn native_fixture_with(service: bool, acknowledge: bool) -> NativeFixture {
    let original = "unit-spa-full-refusal";
    let method = "tasks.update";
    let accepted = super::super::saved_evidence::empty_rejections_control(&[original]);
    let mut slot = super::super::super::trial_complete::make_semantic_slots(TrialName {
        envelope: Envelope::Small,
        number: 1,
    })
    .remove(0);
    slot.original_id = original.into();
    slot.method = Some(method.into());
    slot.due_us = 20_000;
    let observe = Observation::for_native_pump_control(std::slice::from_ref(&slot));
    let intended = observe.original_due(&slot);
    let watch = ClientWatch::new(Arc::new(Instant::now));
    watch.register_relative_at(original, Duration::ZERO, TrafficCohort::SemanticWrite);
    let guard = watch.watch();
    watch.start_at(intended);
    std::thread::sleep(intended.saturating_duration_since(Instant::now()));
    watch.offer(original);
    let offered = Instant::now();
    let sent = Instant::now();
    watch.sent(original);
    let registry = WriteWaitRegistry::new();
    let mut cursor = registry.completed_snapshots_cursor();
    let root = registry.begin(original, method, registry.now());
    root.admitted();
    if service {
        let span = root
            .context()
            .interval(crate::timing::write_wait::IntervalCategory::OwnExternal);
        std::thread::yield_now();
        drop(span);
    }
    if acknowledge {
        root.acknowledged();
    }
    let decoded = Instant::now();
    watch.decoded_final_at(original, FinalOutcome::ProtocolError, decoded);
    root.cancel();
    let native = registry
        .completed_snapshots_since(&mut cursor, 8)
        .unwrap()
        .pop()
        .unwrap();
    watch.server_registered_at(original, native.intake);
    watch.admitted_at(original, native.admitted_at.unwrap());
    let report = guard.finish();
    for (stage, time) in [
        (Stage::Offered, offered),
        (Stage::DispatchStarted, offered),
        (Stage::WireSent, sent),
        (Stage::ServerIntake, native.intake),
        (Stage::Admitted, native.admitted_at.unwrap()),
        (Stage::ParsedFinalReply, decoded),
        (Stage::Completed, Instant::now()),
    ] {
        observe.stage(original, stage, time).unwrap();
    }
    observe
        .union(
            original,
            UnionTrace {
                write_ids: vec![format!("{:?}", native.id)],
                complete: false,
                known_wait_union_us: native.known_wait_union.as_micros() as u64,
                sql_service_union_us: native.sql_service_union.as_micros() as u64,
                blocked_or_sql_union_us: native.blocked_or_sql_union.as_micros() as u64,
                conservative_upper_us: native.conservative_upper.as_micros() as u64,
                own_external_union_us: native.own_external_union.as_micros() as u64,
                max_chargeable_interval_us: native.max_chargeable_interval.as_micros() as u64,
                ..UnionTrace::default()
            },
        )
        .unwrap();
    let raw = ReplyProof {
        original_id: original.into(),
        method: method.into(),
        reply: json!({"id":original,"ok":false,"error_code":"busy","retryable":true}),
        sent_at: sent,
        final_reply_at: decoded,
    };
    let candidate = ControlRefusal {
        original_id: original.into(),
        method: method.into(),
        raw_response_artifact: "explicit-unit-busy-literal.json".into(),
        sent_to_decoded_final_ns: decoded.duration_since(sent).as_nanos() as u64,
        intended_to_decoded_final_ns: decoded.duration_since(intended).as_nanos() as u64,
        decoded_response: raw.reply.clone(),
        native_terminal_rows: vec![super::super::native_archive::native_row(
            &native,
            Instant::now(),
        )],
        acceptance_records: Some(vec![]),
        evidence_errors: vec![],
        pending_pre_durable_verification: true,
    };
    let audit = RefusalAudit::new([(slot.clone(), method.into())]).unwrap();
    assert!(audit.quarantine(&native));
    audit
        .candidate(&slot, method, &raw, candidate.clone(), intended)
        .unwrap();
    NativeFixture {
        audit,
        slot,
        registry,
        observe,
        accepted,
        watch,
        watch_report: report,
        candidate,
        raw,
        intended,
    }
}
pub(in super::super) fn verified_spa_full() -> Bundle {
    let fixture = native_fixture();
    let token = fixture
        .audit
        .finish(
            &fixture.registry.snapshot(),
            &fixture.registry.completed_intervals(),
            &fixture.observe,
            Some(&fixture.accepted),
        )
        .unwrap();
    Bundle {
        token,
        plan: vec![fixture.slot],
        watch: fixture.watch,
        watch_report: fixture.watch_report,
    }
}
pub(in super::super) fn verified_zero_refusals() -> Bundle {
    let accepted = super::super::saved_evidence::empty_rejections_control(&[]);
    let registry = WriteWaitRegistry::new();
    let observe = Observation::for_native_pump_control(&[]);
    let audit = RefusalAudit::new([]).unwrap();
    let token = audit
        .finish(
            &registry.snapshot(),
            &registry.completed_intervals(),
            &observe,
            Some(&accepted),
        )
        .unwrap();
    let watch = ClientWatch::new(Arc::new(Instant::now));
    let guard = watch.watch();
    watch.start_at(Instant::now());
    let watch_report = guard.finish();
    Bundle {
        token,
        plan: vec![],
        watch,
        watch_report,
    }
}
