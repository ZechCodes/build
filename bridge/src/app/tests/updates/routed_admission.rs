//! Admission belongs to the complete RPC, including its off-lock continuation.
use super::*;
use crate::timing::{phase_samples::PhaseSamples, FrameClock};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc,
};

mod lifetimes;

const WATCHDOG: Duration = Duration::from_secs(5);

fn passive_handler(app: AppState) -> (Arc<Mutex<AppState>>, FrameHandler, PhaseSamples) {
    let state = Arc::new(Mutex::new(app));
    let handler = AppState::handler(Arc::clone(&state));
    let clock = Arc::clone(&state.lock().unwrap().frame_clock);
    let samples = clock.capture_phases();
    (state, handler, samples)
}

fn presence_uses_one_admission(updates: bool) {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    if updates {
        let service = update_fixture(dir.path(), false);
        assert!(service.admission().try_enter().is_some());
        app = app.with_update_service(service);
    }
    app.frame_clock = FrameClock::reporting_to(Arc::new(|_| {}));
    app.user_session = crate::session_summary::UserSession::default();
    let (state, handler, samples) = passive_handler(app);
    let reply = handler.call(
        SessionSender::detached("arrival"),
        req("user.present", json!({})),
    );
    assert_eq!(reply["ok"], true, "{reply}");
    let app = state.lock().unwrap();
    let session = &reply["result"]["user_session"];
    assert!(session["session_started_ms"].is_i64(), "{reply}");
    assert_eq!(session["last_activity_ms"], session["session_started_ms"]);
    assert_eq!(
        app.user_session.session_started_ms,
        session["session_started_ms"].as_i64()
    );
    assert_eq!(
        app.store.as_ref().unwrap().load_user_session().unwrap(),
        Some(app.user_session)
    );
    let captured = samples.snapshot();
    assert_eq!(
        captured["user.present"]["frame.total"]["count"], 1,
        "{captured}"
    );
    assert_eq!(
        captured["user.present"]["entered.lock"]["count"], 3,
        "one request admission, one dispatch, one queue/maintenance claim: {captured}"
    );
}

#[test]
fn routed_presence_reuses_open_update_admission() {
    presence_uses_one_admission(true);
}

#[test]
fn routed_presence_without_update_service_is_already_admitted() {
    presence_uses_one_admission(false);
}

#[test]
fn deletion_refuses_routed_writes_before_taking_deferred_or_queued_work() {
    for method in ["user.present", "unknown.disallowed.write"] {
        let (dir, repo) = init_repo();
        let (state, handler, _) = passive_handler(qa_state(&repo, dir.path()));
        let ran = Arc::new(AtomicBool::new(false));
        let witness = Arc::clone(&ran);
        let before = {
            let mut app = state.lock().unwrap();
            app.user_session = crate::session_summary::UserSession::default();
            app.delivery_queue.enqueue(pending_turn(&repo));
            app.deferred_work = Some(DeferredWork::External(Box::new(move || {
                witness.store(true, Ordering::SeqCst);
                Ok(json!({"sentinel":method}))
            })));
            app.project_deletion_in_progress = true;
            app.publish_read_metadata();
            app.read_view.load()
        };
        let reply = handler.call(SessionSender::detached("refused"), req(method, json!({})));
        let mut app = state.lock().unwrap();
        let job = app.take_deferred();
        let session = app.user_session;
        let queued = app.delivery_queue.queued_len();
        let unchanged = Arc::ptr_eq(&before, &app.read_view.load());
        app.project_deletion_in_progress = false;
        app.delivery_queue.clear_queued();
        drop(app);
        assert_eq!(reply["ok"], false, "{method}: {reply}");
        assert_eq!(reply["error_code"], "unavailable", "{reply}");
        assert_eq!(
            reply["error"],
            crate::app::rpc::PROJECT_DELETION_IN_PROGRESS,
            "{reply}"
        );
        assert!(
            !ran.load(Ordering::SeqCst),
            "refusal must not run the sentinel"
        );
        assert_eq!(session, crate::session_summary::UserSession::default());
        assert_eq!(queued, 1);
        assert!(unchanged);
        let job = job.expect("refusal must leave the original deferred job available");
        let _ = job.run();
        assert!(ran.load(Ordering::SeqCst));
    }
}

#[tokio::test]
async fn closed_update_gate_refuses_writes_but_exempt_drains_leave_work_queued() {
    let (dir, repo) = init_repo();
    let updates = update_fixture(dir.path(), false);
    updates.check().await.unwrap();
    updates
        .install(crate::update::InstallWhen::Idle, true)
        .await
        .unwrap();
    assert_eq!(
        updates.tick(false).await.unwrap().state,
        UpdateState::Installing
    );
    let mut app = qa_state(&repo, dir.path()).with_update_service(Arc::clone(&updates));
    app.frame_clock = FrameClock::reporting_to(Arc::new(|_| {}));
    app.user_session = crate::session_summary::UserSession::default();
    let (state, handler, samples) = passive_handler(app);
    state
        .lock()
        .unwrap()
        .delivery_queue
        .enqueue(pending_turn(&repo));
    let before = state.lock().unwrap().read_view.load();
    let refused = handler.call(
        SessionSender::detached("blocked"),
        req("user.present", json!({})),
    );
    let ping = call(&handler, "ping", json!({}));
    let status = call(&handler, "bridge.update_status", json!({}));
    let stats = call(&handler, "bridge.stats", json!({}));
    let app = state.lock().unwrap();
    assert_eq!(refused["error_code"], "busy", "{refused}");
    assert_eq!(
        refused["error"], "bridge update handoff in progress",
        "{refused}"
    );
    assert_eq!(
        app.user_session,
        crate::session_summary::UserSession::default()
    );
    assert_eq!(ping["result"], crate::app::rpc::pong());
    assert_eq!(status["result"]["state"], "installing", "{status}");
    assert_eq!(stats["ok"], true, "{stats}");
    assert_eq!(
        app.delivery_queue.queued_len(),
        1,
        "exemption is not permission to drain"
    );
    assert!(Arc::ptr_eq(&before, &app.read_view.load()));
    assert!(updates.admission().try_enter().is_none());
    let captured = samples.snapshot();
    assert!(
        captured["bridge.stats"].get("entered.lock").is_none(),
        "{captured}"
    );
}
