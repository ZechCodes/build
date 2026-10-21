//! Actual failed MCP response versus still-owned socket/server cleanup.
use super::*;
use crate::app::tests::runtime::write_wait_qualification::client_watch::{
    FinalOutcome, TrafficCohort,
};
use crate::app::tests::runtime::write_wait_qualification::mcp_adapter::{
    McpIdentity, McpShutdownReport,
};
use crate::mcp::BridgeAction;
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::Instant;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "native failed MCP cleanup and actual5s owned-tail refusal; explicit NI10 release control"]
async fn failed_mcp_reply_cleans_joined_setup_but_retains_owned_socket_tail() {
    assert!(
        !cfg!(debug_assertions),
        "native cleanup control requires release"
    );
    crate::git_fixture::environment::isolated_git_test!();
    assert_eq!(crate::priority::own_nice(), 10);
    let artifacts = tempfile::Builder::new()
        .prefix("qualification-mcp-cleanup-NoQualification-")
        .tempdir()
        .unwrap()
        .keep();
    eprintln!("mcp_cleanup_control_artifacts={}", artifacts.display());
    // Both actual fixtures are fully cleaned before either target is asserted.
    let mut cases = Vec::new();
    for held_tail in [false, true] {
        cases.push(run_case(held_tail, &artifacts).await);
    }
    for case in &cases {
        assert_eq!(case.reply["ok"], false, "{}", case.reply);
        assert!(case.reply["error"]
            .as_str()
            .is_some_and(|text| !text.is_empty()));
        assert!(
            case.first_failed,
            "failed response must never return cleanup success"
        );
        assert!(
            case.released,
            "native AppState must be released before assertions"
        );
        assert!(
            case.fallback_errors.is_empty(),
            "{:?}",
            case.fallback_errors
        );
        assert!(
            case.pause_timely,
            "owned decoded observer release timed out"
        );
        let final_row = &case.final_mcp.evidence.calls[&case.original];
        assert_eq!(final_row.response_ok, Some(false));
        assert!(final_row.final_reply_at.is_some());
        assert!(final_row.client_closed_at.is_some() && final_row.server_joined_at.is_some());
        assert!(final_row.worker_finished_at.is_some() && final_row.worker_reaped_at.is_some());
        assert!(case.final_mcp.quiet && case.final_mcp.unfinished.is_empty());
        assert!(case
            .final_mcp
            .evidence
            .sockets
            .iter()
            .all(|row| row.active == 0));
        assert!(
            !case.final_mcp.is_clean(),
            "semantic failure remains failed"
        );
        assert_eq!(case.final_watch.rows.len(), 1);
        assert_eq!(
            case.final_watch.rows[0].final_outcome,
            Some(FinalOutcome::ProtocolError)
        );
        assert!(!case.final_watch.violations.is_empty());
        if case.held_tail {
            assert!(case.pause_reached && !case.first_closed);
            assert!(!case.first_observed.quiet);
            assert_eq!(case.first_observed.unfinished, vec![case.original.clone()]);
            assert!(case
                .first_observed
                .evidence
                .sockets
                .iter()
                .any(|row| row.active == 1));
            let held = &case.first_observed.evidence.calls[&case.original];
            assert!(held.server_joined_at.is_none() && held.worker_reaped_at.is_none());
            assert!(case.first_elapsed >= Duration::from_secs(5));
        }
    }
    // Both cases' real cleanup and the still-owned negative guard precede the
    // baseline target RED: is_clean incorrectly prevents quiet failed cleanup.
    assert!(
        cases[0].first_closed,
        "joined failed MCP reply must permit native cleanup: {}",
        cases[0].first_reason
    );
    for case in &cases {
        let retained = case
            .first_retained
            .as_ref()
            .expect("actual MCP shutdown report must remain retained");
        assert_eq!(retained.quiet, case.first_observed.quiet);
        assert_eq!(retained.unfinished, case.first_observed.unfinished);
        assert_eq!(retained.evidence.calls.len(), 1);
        assert!(!retained.is_clean());
    }
}

struct Case {
    held_tail: bool,
    original: String,
    reply: serde_json::Value,
    first_failed: bool,
    first_closed: bool,
    first_reason: String,
    first_elapsed: Duration,
    first_observed: McpShutdownReport,
    first_retained: Option<McpShutdownReport>,
    final_mcp: McpShutdownReport,
    final_watch: super::super::super::client_watch::FinalReconciliation,
    pause_reached: bool,
    pause_timely: bool,
    released: bool,
    fallback_errors: Vec<String>,
}

async fn run_case(held_tail: bool, artifacts: &std::path::Path) -> Case {
    let original = format!("mcp-cleanup-control/{held_tail}/get_task");
    let prepared = control_owner(&original).await;
    let watch = prepared.watch.as_ref().unwrap().clone();
    let owner = Arc::clone(prepared.lifecycle.as_ref().unwrap());
    let weak = Arc::downgrade(&prepared.fixture.as_ref().unwrap().state);
    let (arrival, release, pause_timely) = observe_tail(&watch, held_tail);
    watch.offer(&original);
    let mcp = Arc::clone(prepared.mcp.as_ref().unwrap());
    let call_id = original.clone();
    let call = tokio::spawn(async move {
        mcp.execute(
            0,
            &call_id,
            BridgeAction::TrackerGetTask {
                task_id: "missing-mcp-cleanup-control-task".into(),
            },
            true,
        )
        .await
    });
    let mut call = Some(call);
    let mut proof = None;
    let pause_reached = if held_tail {
        tokio::time::timeout(Duration::from_secs(2), arrival)
            .await
            .is_ok_and(|r| r.is_ok())
    } else {
        proof = Some(call.take().unwrap().await);
        false
    };
    let started = Instant::now();
    let first = prepared.shutdown().await;
    let first_elapsed = started.elapsed();
    let (first_failed, first_reason, mut retained) = match first {
        Ok(_) => (false, "unexpected successful cleanup".into(), None),
        Err(failure) => (true, failure.reason, Some(failure.retained)),
    };
    let first_closed = retained
        .as_ref()
        .is_none_or(|p| p.fixture.is_none() && p.mcp.is_none());
    let first_retained = retained.as_ref().and_then(|p| p.mcp_shutdown.clone());
    // Observe the same actual MCP owner without resetting its elapsed deadline.
    // Before the barrier release this must still contain the owned server.
    let first_observed = if let Some(mcp) = retained.as_ref().and_then(|p| p.mcp.as_ref()) {
        mcp.shutdown(tokio::time::Instant::now()).await
    } else {
        first_retained
            .clone()
            .expect("released MCP must retain its real report")
    };
    let _ = release.send(());
    if let Some(call) = call {
        proof = Some(call.await);
    }
    let final_mcp = if let Some(mcp) = retained.as_ref().and_then(|p| p.mcp.as_ref()) {
        // Failure-only recovery after explicit barrier release; this is not a
        // continuation or reset of the immutable first cleanup observation.
        mcp.shutdown(tokio::time::Instant::now() + Duration::from_secs(5))
            .await
    } else {
        first_observed.clone()
    };
    let mut fallback_errors = Vec::new();
    if !final_mcp.quiet
        || !final_mcp.unfinished.is_empty()
        || final_mcp.evidence.calls.values().any(|row| {
            row.client_closed_at.is_none()
                || row.server_joined_at.is_none()
                || row.worker_finished_at.is_none()
                || row.worker_reaped_at.is_none()
        })
    {
        // No accepted owner is dropped to make this control finish.
        if let Some(retained) = retained.take() {
            std::mem::forget(retained);
        }
        panic!("MCP failure recovery still owns resources: {final_mcp:?}");
    }
    owner.request_finish();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !owner.is_finished() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    if !owner.is_finished() || owner.report().is_none_or(|report| report.panicked) {
        fallback_errors.push("actual MCP watch join unconfirmed".into());
    }
    if let Some(mut retained) = retained {
        retained.mcp.take();
        if let Some(fixture) = retained.fixture.take() {
            fallback_errors.extend(fixture.cleanup_owned_resources().await);
        }
    }
    let until = Instant::now() + Duration::from_secs(10);
    while weak.upgrade().is_some() && Instant::now() < until {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let released = weak.upgrade().is_none();
    let final_watch = watch.reconcile_final();
    let proof = proof
        .unwrap()
        .expect("owned MCP caller joined")
        .expect("actual decoded MCP reply");
    std::fs::write(artifacts.join(format!("case-{held_tail}.json")), serde_json::to_vec_pretty(&json!({
        "qualification":false,"normal_originals_enrolled":0,"held_tail":held_tail,
        "original_id":original,"reply":proof.reply,"first_failed":first_failed,
        "first_closed":first_closed,"first_reason":first_reason,"first_elapsed_us":first_elapsed.as_micros(),
        "first_observed":format!("{first_observed:?}"),"first_retained":format!("{first_retained:?}"),
        "final_mcp":format!("{final_mcp:?}"),"final_watch":format!("{final_watch:?}"),
        "released":released,"fallback_errors":fallback_errors,
    })).unwrap()).unwrap();
    Case {
        held_tail,
        original,
        reply: proof.reply,
        first_failed,
        first_closed,
        first_reason,
        first_elapsed,
        first_observed,
        first_retained,
        final_mcp,
        final_watch,
        pause_reached,
        pause_timely: pause_timely.load(Ordering::Acquire),
        released,
        fallback_errors,
    }
}

async fn control_owner(original: &str) -> PreparedBackend {
    let mut prepared = PreparedBackend::unassembled(
        TrialName {
            envelope: Envelope::Small,
            number: 1,
        },
        PipelineMode::TwoSecondControl,
        super::super::preparation_tests::marker(),
    )
    .await;
    // Exactly one explicit control call, no full59 or normal original enrollment.
    let watch = ClientWatch::new(Arc::new(Instant::now));
    watch.register_cohort_at(original, Instant::now(), TrafficCohort::Read);
    let owner = watch_owner::WatchOwner::new(watch.watch());
    let fixture = prepared.fixture.as_ref().unwrap();
    let identities = fixture
        .active_workspaces
        .iter()
        .map(|workspace| McpIdentity {
            agent: workspace.agent_id.clone(),
            token: format!("qual-token-{}", workspace.agent_id),
            read_only: false,
        })
        .collect();
    prepared.mcp = Some(
        McpHarness::pipeline_control(
            Arc::clone(&fixture.state),
            Arc::clone(&fixture.registry),
            identities,
            watch.clone(),
        )
        .unwrap(),
    );
    prepared.watch = Some(watch);
    prepared.lifecycle = Some(owner);
    prepared
}

fn observe_tail(
    watch: &ClientWatch,
    held: bool,
) -> (
    tokio::sync::oneshot::Receiver<()>,
    mpsc::Sender<()>,
    Arc<AtomicBool>,
) {
    let (arrived, arrival) = tokio::sync::oneshot::channel();
    let arrived = Mutex::new(Some(arrived));
    let (release, wait) = mpsc::channel();
    let wait = Mutex::new(wait);
    let timely = Arc::new(AtomicBool::new(true));
    let result = Arc::clone(&timely);
    watch
        .observe_decoded_replies(Arc::new(move |_, _| {
            if let Some(arrived) = arrived.lock().unwrap().take() {
                let _ = arrived.send(());
                if held
                    && wait
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(8))
                        .is_err()
                {
                    timely.store(false, Ordering::Release);
                }
            }
        }))
        .unwrap();
    (arrival, release, result)
}
