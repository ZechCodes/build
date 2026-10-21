//! Reuse native tracker fixtures and native setup. No wire/qualification claim.
use super::super::super::common_requests::ActorExpectation;
use super::super::super::trial_complete::{Envelope, Route};
use super::*;
use crate::app::tests::tracker_tracking::optional_notices::Fixture;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "actual setup collector control; serial NI10 release only, no qualification traffic"]
async fn native_optional_notice_setup_captures_exact_plan_and_zero_cohort() {
    crate::git_fixture::environment::isolated_git_test!();
    assert_eq!(crate::priority::own_nice(), 10);
    let name = TrialName {
        envelope: Envelope::Small,
        number: 1,
    };
    let mut prepared = PreparedBackend::unassembled(
        name,
        PipelineMode::TwoSecondControl,
        super::super::preparation_tests::marker(),
    )
    .await;
    use futures_util::FutureExt;
    let setup = std::panic::AssertUnwindSafe(prepared.assemble())
        .catch_unwind()
        .await;
    let expected: Vec<_> = prepared
        .plan
        .iter()
        .filter_map(|slot| {
            let request = prepared.common.get(&slot.original_id)?;
            matches!(&request.effect, ExpectedEffect::TaskStatus { .. })
                .then(|| slot.original_id.clone())
        })
        .collect();
    let mut evidence = prepared.optional_notices.take();
    let snapshot = evidence.as_mut().map(|evidence| {
        let fixture = prepared.fixture.as_ref().unwrap();
        evidence.freeze_before_cleanup()?;
        evidence.cancel_queued(&mut fixture.state.lock().unwrap())
    });
    // Missing capture/assertion must not skip actual fixture cleanup.
    let cleanup = prepared.shutdown().await;
    let clean = match cleanup {
        Ok(receipt) => {
            receipt.producers_joined
                && receipt.mcp_quiet
                && receipt.client_watch_joined
                && receipt.native_cleanup_completed
        }
        Err(failure) => {
            let reason = failure.reason;
            std::mem::forget(failure.retained);
            panic!("actual setup cleanup failed; ownership retained: {reason}");
        }
    };
    assert!(matches!(setup, Ok(Ok(()))), "setup failed: {setup:?}");
    assert!(clean);
    assert!(
        !expected.is_empty(),
        "actual common plan includes status mutations"
    );
    let evidence =
        evidence.expect("actual prepare_collectors must install optional notice capture before T0");
    assert_eq!(snapshot, Some(Ok(0)));
    assert_eq!(
        evidence
            .declared
            .iter()
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>(),
        expected
    );
    assert!(evidence
        .declared
        .iter()
        .all(|(_, kind)| *kind == Kind::TrackerWatchNotice));
    let report = evidence.report(true);
    assert_eq!(report["declared_enrollment_counts"]["CompleteReminder"], 0);
    for phase in ["before_cleanup", "after_cleanup_attempt"] {
        assert_counts(&report[phase], 0, 0, 0, 0, 0);
        assert_eq!(
            report[phase]["completed_lifetime_us"],
            json!({"count":0,"p95":0,"max":0})
        );
        assert_eq!(
            report[phase]["active_age_us"],
            json!({"censored":true,"count":0,"p95":0,"max":0})
        );
        assert!(report[phase]["samples"].as_array().unwrap().is_empty());
    }
}

#[test]
fn native_optional_report_preserves_off_on_completed_and_censored_cleanup_counts() {
    for enabled in [false, true] {
        let mut fixture = Fixture::new();
        let mut evidence = capture(&fixture, "report-off-on", enabled);
        let context = fixture.update("report-off-on");
        assert!(fixture
            .state
            .origin_publication_complete(&context, &fixture.watcher.0));
        fixture.advance(6);
        let required_before = fixture.row("report-off-on");
        evidence.freeze_before_cleanup().unwrap();
        let removed = evidence.cancel_queued(&mut fixture.state);
        let before = evidence.before_cleanup.clone();
        fixture.finish();
        let report = evidence.report(false);
        assert_eq!(report["enabled"], enabled);
        if enabled {
            assert_eq!(removed, Ok(1));
            assert!(required_before.outstanding.is_empty());
            let sample = &evidence.policy.samples()[0];
            assert_ne!(sample.delivery.original_id, "report-off-on");
            assert!(sample.delivery.original_id.starts_with("optional-notice/"));
            assert_eq!(sample.original_ids, ["report-off-on"]);
            assert_counts(before.as_ref().unwrap(), 1, 1, 0, 1, 0);
            assert_eq!(
                report["before_cleanup"]["active_age_us"],
                json!({"censored":true,"count":1,"p95":6_000_000,"max":6_000_000})
            );
            assert_eq!(
                report["before_cleanup"]["completed_lifetime_us"],
                json!({"count":0,"p95":0,"max":0})
            );
            assert_counts(&report["after_cleanup_attempt"], 1, 1, 0, 0, 1);
            assert_eq!(
                report["after_cleanup_attempt"]["samples"][0]["cancelled"],
                true
            );
            assert_eq!(
                report["before_cleanup"]["samples"][0]["original_ids"],
                json!(["report-off-on"])
            );
            assert_eq!(report["before_cleanup"]["samples"][0]["censored"], true);
        } else {
            assert!(removed.is_err());
            assert!(required_before.breached() && !required_before.outstanding.is_empty());
            assert_counts(&report["before_cleanup"], 1, 0, 0, 0, 0);
            assert_counts(&report["after_cleanup_attempt"], 1, 0, 0, 0, 0);
            assert!(report["queue_cleanup"]["error"].is_string());
        }
        assert_eq!(
            report["after_cleanup_attempt"]["created_by_kind"]["CompleteReminder"],
            0
        );
    }
    let mut fixture = Fixture::new();
    let mut evidence = capture(&fixture, "report-completed", true);
    fixture.update("report-completed");
    fixture.advance(6);
    fixture.finish();
    evidence.freeze_before_cleanup().unwrap();
    assert_eq!(evidence.cancel_queued(&mut fixture.state), Ok(0));
    let report = evidence.report(false);
    for phase in ["before_cleanup", "after_cleanup_attempt"] {
        assert_counts(&report[phase], 1, 1, 1, 0, 0);
        assert_eq!(
            report[phase]["completed_lifetime_us"],
            json!({"count":1,"p95":6_000_000,"max":6_000_000})
        );
        assert_eq!(
            report[phase]["active_age_us"],
            json!({"censored":true,"count":0,"p95":0,"max":0})
        );
        assert_eq!(report[phase]["samples"][0]["completed"], true);
    }
}

#[tokio::test]
async fn native_optional_report_cleanup_preserves_mixed_and_running_turns() {
    let mut mixed = Fixture::new();
    let mut evidence = capture(&mixed, "report-optional-mixed", true);
    mixed.update("report-optional-mixed");
    let required =
        mixed
            .registry
            .begin("report-required-mixed", "thread.post", mixed.registry.now());
    {
        let _bound = required.context().bind_sync();
        let reply = mixed.state.handle(crate::app::tests::req("thread.post", json!({
            "entity_id":mixed.watcher.0,"agent_id":mixed.watcher.1,"body":"required work remains charged"
        })));
        assert_eq!(reply["ok"], true);
    }
    required.complete();
    mixed.advance(6);
    evidence.freeze_before_cleanup().unwrap();
    assert!(evidence.cancel_queued(&mut mixed.state).is_err());
    let required = mixed.row("report-required-mixed");
    mixed.finish();
    let report = evidence.report(false);
    assert!(required.breached() && !required.cancelled && !required.outstanding.is_empty());
    assert_eq!(report["before_cleanup"]["active"], 1);
    assert_eq!(report["after_cleanup_attempt"]["cancelled_for_cleanup"], 0);
    assert!(report["queue_cleanup"]["error"].is_string());

    let mut running = Fixture::new();
    let mut evidence = capture(&running, "report-running", true);
    running.update("report-running");
    running.state.delivery_queue.lapse_settle_windows();
    let mut turns = running.state.delivery_queue.take_ready(|_| false);
    assert_eq!(turns.len(), 1);
    running.advance(6);
    evidence.freeze_before_cleanup().unwrap();
    assert_eq!(evidence.cancel_queued(&mut running.state), Ok(0));
    let deadline = tokio::time::Instant::now();
    let blocked = wait_running_notices(&running.registry, deadline).await;
    let report = evidence.report(false);
    // Only the real running turn's completion drains its child. The cleanup
    // boundary neither cancels it nor extends its original deadline.
    for turn in &mut turns {
        turn.complete_logical_origins();
    }
    let drained = wait_running_notices(&running.registry, deadline).await;
    assert!(blocked
        .unwrap_err()
        .contains("running optional notices did not drain"));
    assert_eq!(drained, Ok(()));
    assert_counts(&report["before_cleanup"], 1, 1, 0, 1, 0);
    assert_counts(&report["after_cleanup_attempt"], 1, 1, 0, 1, 0);
    assert_eq!(
        report["after_cleanup_attempt"]["samples"][0]["cancelled"],
        false
    );
    assert_eq!(report["before_cleanup"]["active_age_us"]["p95"], 6_000_000);
}

fn capture(fixture: &Fixture, original: &str, enabled: bool) -> OptionalNoticeEvidence {
    let slot = Slot {
        original_id: original.into(),
        sequence: 0,
        traffic: Traffic::Write,
        route: Route::SpaRpc,
        caller: 0,
        owner: 0,
        due_us: 0,
        class: None,
        finite_target_us: None,
        finite_ceiling_us: None,
        method: Some("tasks.update".into()),
    };
    let request = CommonRequest {
        original_id: original.into(),
        caller: 0,
        owner: 0,
        request: ActualRequest::Rpc {
            method: "tasks.update",
            params: json!({"task_id":fixture.task,"status":"ready"}),
        },
        effect: ExpectedEffect::TaskStatus {
            task: fixture.task.clone(),
            status: "ready".into(),
            actor: ActorExpectation::User,
        },
        durable_operation_id: None,
        predecessor: None,
    };
    OptionalNoticeEvidence::capture(
        &fixture.state,
        &[slot],
        &BTreeMap::from([(original.into(), request)]),
        enabled,
    )
    .unwrap()
}
fn assert_counts(
    value: &Value,
    offered: usize,
    created: usize,
    completed: usize,
    active: usize,
    cancelled: usize,
) {
    assert_eq!(value["offered"], offered);
    assert_eq!(value["created"], created);
    assert_eq!(value["completed"], completed);
    assert_eq!(value["active"], active);
    assert_eq!(value["cancelled_for_cleanup"], cancelled);
}
