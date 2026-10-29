//! Exercise the shared entry, real dispatcher and exact output lifetimes.

use super::*;
use crate::execution::session::{
    DeliveryError, EncodedEvent, EventBinding, EventSink, HttpIdentity, ReplySink, StreamClass,
    VerifiedCaller,
};
use crate::timing::FrameClock;
use serde_json::json;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc as blocking;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::{timeout, Instant};

const WATCHDOG: Duration = Duration::from_secs(10);

struct Sink {
    output: mpsc::UnboundedSender<EncodedEvent>,
    dropped: Option<oneshot::Sender<()>>,
}

impl EventSink for Sink {
    fn try_send(&self, event: EncodedEvent) -> Result<(), DeliveryError> {
        self.output.send(event).map_err(|_| DeliveryError::Closed)
    }
}
impl ReplySink for Sink {
    fn try_send(&self, event: EncodedEvent) -> Result<(), DeliveryError> {
        EventSink::try_send(self, event)
    }
}
impl Drop for Sink {
    fn drop(&mut self) {
        if let Some(dropped) = self.dropped.take() {
            let _ = dropped.send(());
        }
    }
}

fn sink() -> (
    Arc<Sink>,
    mpsc::UnboundedReceiver<EncodedEvent>,
    oneshot::Receiver<()>,
) {
    let (output, received) = mpsc::unbounded_channel();
    let (dropped, gone) = oneshot::channel();
    (
        Arc::new(Sink {
            output,
            dropped: Some(dropped),
        }),
        received,
        gone,
    )
}

fn caller(live: Arc<AtomicBool>, deadline: Instant) -> VerifiedCaller {
    VerifiedCaller::verified_http(
        HttpIdentity {
            issuer: "https://account.example".into(),
            account_id: "owner".into(),
            device_id: "device".into(),
            family_id: "family".into(),
            origin: "https://device.example".into(),
            endpoint_epoch: "endpoint-1".into(),
            authorization_epoch: 1,
        },
        deadline,
        Arc::new(move || live.load(Ordering::SeqCst)),
    )
}

fn open(service: &ExecutionService) -> SessionLease {
    service
        .sessions()
        .open(caller(
            Arc::new(AtomicBool::new(true)),
            Instant::now() + Duration::from_secs(60),
        ))
        .unwrap()
}

async fn submit(
    service: &ExecutionService,
    session: &SessionLease,
    events: Option<&EventBinding>,
    sink: Arc<dyn ReplySink>,
    method: &str,
) -> Call {
    let request = service.sessions().request(session, sink, events).unwrap();
    timeout(
        WATCHDOG,
        service.submit(request, Invocation::decode(method, &json!({})).unwrap()),
    )
    .await
    .expect("shared admission must settle")
    .unwrap()
}

async fn reply(call: &mut Call) -> Value {
    timeout(WATCHDOG, call.reply())
        .await
        .expect("shared worker must settle")
        .expect("live request must receive its final reply")
}

/// One worker can be held inside its actual blocking handler. Subsequent work
/// is deterministically queued until the test releases the first invocation.
fn blocked_handler() -> (
    FrameHandler,
    mpsc::UnboundedReceiver<CallContext>,
    blocking::Sender<()>,
    Arc<AtomicUsize>,
) {
    let (entered, arrived) = mpsc::unbounded_channel();
    let (release, released) = blocking::channel();
    let released = Mutex::new(released);
    let effects = Arc::new(AtomicUsize::new(0));
    let witnessed = effects.clone();
    let handler = FrameHandler::new(FrameClock::new(), move |sender, frame, _| {
        if witnessed.fetch_add(1, Ordering::SeqCst) == 0 {
            entered.send(sender.event_context()).unwrap();
            released
                .lock()
                .unwrap()
                .recv_timeout(WATCHDOG)
                .expect("test must release the running handler");
        }
        json!({"id":frame.payload["id"],"ok":true,"result":{"served":true}})
    });
    (handler, arrived, release, effects)
}

fn single_worker(handler: FrameHandler) -> Arc<ExecutionService> {
    ExecutionService::from_dispatcher(Arc::new(Dispatcher::with_capacity(handler, 8, 1)))
}

#[tokio::test(start_paused = true)]
async fn revoked_or_expired_authority_stops_snapshot_before_handler_or_output() {
    for expire in [false, true] {
        let snapshots = Arc::new(AtomicUsize::new(0));
        let witnessed = snapshots.clone();
        let handler = FrameHandler::new(FrameClock::new(), |_, _, _| {
            panic!("unauthorized request entered application dispatch")
        })
        .with_snapshot_reads(move |frame| {
            witnessed.fetch_add(1, Ordering::SeqCst);
            json!({"id":frame.payload["id"],"ok":true,"result":{}})
        });
        let service = ExecutionService::new(handler);
        let live = Arc::new(AtomicBool::new(true));
        let session = service
            .sessions()
            .open(caller(
                live.clone(),
                Instant::now() + Duration::from_secs(1),
            ))
            .unwrap();
        let (output, mut received, _gone) = sink();
        let request = service.sessions().request(&session, output, None).unwrap();
        if expire {
            tokio::time::advance(Duration::from_secs(2)).await;
        } else {
            live.store(false, Ordering::SeqCst);
        }
        let outcome = service
            .submit(
                request,
                Invocation::decode("board.list", &json!({})).unwrap(),
            )
            .await;
        assert!(matches!(outcome, Err(SessionError::Unauthorized)));
        assert_eq!(snapshots.load(Ordering::SeqCst), 0);
        assert!(
            received.try_recv().is_err(),
            "no reply, receipt or payload may escape"
        );
    }
}

#[tokio::test]
async fn event_replacement_fences_running_producer_without_cancelling_reply() {
    let (handler, mut entered, release, _) = blocked_handler();
    let service = single_worker(handler);
    let session = open(&service);
    let (old_sink, mut old_output, _old_gone) = sink();
    let old = service
        .sessions()
        .bind_events(&session, StreamClass::Application, old_sink)
        .unwrap();
    let (response, mut responses, _response_gone) = sink();
    let mut call = submit(&service, &session, Some(&old), response, "user.present").await;
    let delayed = timeout(WATCHDOG, entered.recv()).await.unwrap().unwrap();
    let (new_sink, mut new_output, _new_gone) = sink();
    let fresh = service
        .sessions()
        .replace_events(&session, StreamClass::Application, Some(&old), new_sink)
        .unwrap();
    let stale_sent = delayed.push(json!({"type":"models.changed","stale":true}));
    old.close();
    let fresh_sent = fresh.events().push(json!({"type":"models.changed"}));
    // Release blocking work before asserting observations, including failures.
    release.send(()).unwrap();
    let answer = reply(&mut call).await;
    assert!(
        !stale_sent,
        "old producer reached the replacement event stream"
    );
    fresh_sent.unwrap();
    assert!(old_output.try_recv().is_err());
    assert!(new_output.try_recv().unwrap().is_live());
    assert_eq!(answer["ok"], true);
    assert!(timeout(WATCHDOG, responses.recv())
        .await
        .unwrap()
        .unwrap()
        .is_live());
}

#[tokio::test]
async fn cancelling_running_call_retains_reply_owned_budget_until_handler_finishes() {
    let (handler, mut entered, release, _) = blocked_handler();
    let service = single_worker(handler);
    let session = open(&service);
    let (response, mut output, mut dropped) = sink();
    let call = submit(&service, &session, None, response, "user.present").await;
    let producer = timeout(WATCHDOG, entered.recv()).await.unwrap().unwrap();
    drop(call);
    let retained = matches!(dropped.try_recv(), Err(oneshot::error::TryRecvError::Empty));
    release.send(()).unwrap();
    timeout(WATCHDOG, &mut dropped)
        .await
        .expect("finished work must release the response budget")
        .unwrap();
    assert!(
        retained,
        "request drop released the budget while effects were running"
    );
    assert!(
        output.try_recv().is_err(),
        "cancelled reply must stay suppressed"
    );
    // A retained producer context must not extend that request's sink lifetime.
    drop(producer);
}

#[tokio::test]
async fn cancelled_queued_mutation_starts_no_effect_and_releases_its_budget() {
    let (handler, mut entered, release, effects) = blocked_handler();
    let service = single_worker(handler);
    let session = open(&service);
    let (blocking_sink, _blocking_output, _blocking_dropped) = sink();
    let mut blocking = submit(&service, &session, None, blocking_sink, "user.present").await;
    let _producer = timeout(WATCHDOG, entered.recv()).await.unwrap().unwrap();
    let (queued_sink, mut queued_output, queued_dropped) = sink();
    let queued = submit(&service, &session, None, queued_sink, "user.present").await;
    drop(queued);
    release.send(()).unwrap();
    assert_eq!(reply(&mut blocking).await["ok"], true);
    let (sentinel_sink, _sentinel_output, _sentinel_dropped) = sink();
    let mut sentinel = submit(&service, &session, None, sentinel_sink, "user.present").await;
    assert_eq!(reply(&mut sentinel).await["ok"], true);
    timeout(WATCHDOG, queued_dropped)
        .await
        .expect("dead queued caller must release its budget")
        .unwrap();
    assert_eq!(
        effects.load(Ordering::SeqCst),
        2,
        "cancelled queue entry ran an effect"
    );
    assert!(queued_output.try_recv().is_err());
}

async fn ended_queued_authority_starts_no_effect_and_releases_budget(expire: bool) {
    let (handler, mut entered, release, effects) = blocked_handler();
    let service = single_worker(handler);
    let live = Arc::new(AtomicBool::new(true));
    let session = service
        .sessions()
        .open(caller(
            live.clone(),
            Instant::now() + Duration::from_secs(60),
        ))
        .unwrap();
    let (blocking_sink, mut blocking_output, blocking_dropped) = sink();
    let mut blocking = submit(&service, &session, None, blocking_sink, "user.present").await;
    let _producer = timeout(WATCHDOG, entered.recv()).await.unwrap().unwrap();
    let (queued_sink, mut queued_output, queued_dropped) = sink();
    let mut queued = submit(&service, &session, None, queued_sink, "user.present").await;
    if expire {
        // Pause only once the actual worker is occupied and the next request
        // is queued. Setup/admission cannot auto-advance the grant deadline.
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(61)).await;
        tokio::time::resume();
    } else {
        live.store(false, Ordering::SeqCst);
    }
    release.send(()).unwrap();
    // Keep both Calls alive until the worker has consumed their interest.
    // Dropping the queued Call sooner would let cancellation hide a missing
    // authorization check at execution. Already-running work may settle, but
    // revoked/expired authority must suppress its output as well.
    let running_outcome = timeout(WATCHDOG, blocking.reply()).await;
    let queued_outcome = timeout(WATCHDOG, queued.reply()).await;
    drop(blocking);
    drop(queued);
    timeout(WATCHDOG, async {
        blocking_dropped.await.unwrap();
        queued_dropped.await.unwrap();
    })
    .await
    .expect("ended callers must release their response budgets after work drains");
    assert!(
        running_outcome
            .expect("running work must settle after release")
            .is_err(),
        "ended authority must suppress the already-running request's reply"
    );
    assert!(
        queued_outcome
            .expect("queued work must drain without starting")
            .is_err(),
        "ended authority must suppress the queued request's reply"
    );
    assert_eq!(
        effects.load(Ordering::SeqCst),
        1,
        "a request whose authority ended while queued started an effect"
    );
    assert!(blocking_output.try_recv().is_err());
    assert!(queued_output.try_recv().is_err());
}

#[tokio::test]
async fn expired_queued_request_starts_no_effect_and_releases_its_budget() {
    ended_queued_authority_starts_no_effect_and_releases_budget(true).await;
}

#[tokio::test]
async fn revoked_queued_request_starts_no_effect_and_releases_its_budget() {
    ended_queued_authority_starts_no_effect_and_releases_budget(false).await;
}

#[tokio::test]
async fn cancelling_newest_folded_caller_still_serves_the_live_older_caller() {
    let (handler, mut entered, release, effects) = blocked_handler();
    let service = single_worker(handler);
    let session = open(&service);
    let (blocking_sink, _blocking_output, _blocking_dropped) = sink();
    let mut blocking = submit(&service, &session, None, blocking_sink, "user.present").await;
    let _producer = timeout(WATCHDOG, entered.recv()).await.unwrap().unwrap();
    let (older_sink, mut older_output, _older_dropped) = sink();
    let mut older = submit(&service, &session, None, older_sink, "board.list").await;
    let (newer_sink, mut newer_output, _newer_dropped) = sink();
    let newer = submit(&service, &session, None, newer_sink, "board.list").await;
    let folds = service.dispatcher.folded_reads_waiting();
    drop(newer);
    release.send(()).unwrap();
    assert_eq!(reply(&mut blocking).await["ok"], true);
    assert_eq!(folds, 1, "requests must share one queued computation");
    let answer = reply(&mut older).await;
    assert_eq!(answer["ok"], true);
    assert_eq!(
        effects.load(Ordering::SeqCst),
        2,
        "surviving fold must compute once"
    );
    assert!(timeout(WATCHDOG, older_output.recv())
        .await
        .unwrap()
        .unwrap()
        .is_live());
    assert!(newer_output.try_recv().is_err());
}

#[tokio::test]
async fn webrtc_intake_and_shared_entry_consume_the_same_dispatch_worker() {
    use crate::carrier::testing::{client_request, session_init, ChannelWire};
    use crate::carrier::{FrameIntake, SessionSender};

    let (handler, mut entered, release, effects) = blocked_handler();
    let keys = crate::transport::generate_transport_keypair();
    let intake = FrameIntake::with_pool(handler, keys.clone(), 8, 1);
    let service = intake.execution();
    assert!(Arc::ptr_eq(&service, &intake.execution()));
    let session = open(&service);
    let mut wire = ChannelWire::open();
    let key = crate::transport::generate_session_key();
    let init = session_init("wire-service", &keys.public_key_b64, &key);
    wire.open_session(&intake, "wire-service", &init).unwrap();
    timeout(WATCHDOG, wire.outbound.recv())
        .await
        .unwrap()
        .unwrap();
    wire.accept(
        &intake,
        client_request(
            &key,
            "wire-service",
            "data",
            json!({"id":"wire-call","method":"user.present","params":{}}),
        ),
    )
    .await
    .unwrap();
    let _producer = timeout(WATCHDOG, entered.recv()).await.unwrap().unwrap();
    let (output, _received, _gone) = sink();
    let mut shared = submit(&service, &session, None, output, "user.present").await;
    let before_release = effects.load(Ordering::SeqCst);
    release.send(()).unwrap();
    assert_eq!(reply(&mut shared).await["ok"], true);
    let legacy = timeout(WATCHDOG, async {
        loop {
            let encrypted = wire.outbound.recv().await.unwrap();
            let payload = SessionSender::decrypt_push(&key, &encrypted);
            if payload.get("ok").is_some() {
                break payload;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(
        before_release, 1,
        "shared adapter escaped the occupied WebRTC worker"
    );
    assert_eq!(legacy["id"], "wire-call");
    assert_eq!(legacy["ok"], true);
    assert_eq!(effects.load(Ordering::SeqCst), 2);
}

struct UpdateBackend;

#[async_trait::async_trait]
impl crate::update::UpdateBackend for UpdateBackend {
    async fn latest(&self) -> Result<crate::update::Release, String> {
        Ok(crate::update::Release {
            version: "0.3.0".into(),
            tag: "bridge-v0.3.0".into(),
            published_at: None,
        })
    }
    async fn install(&self, _: &crate::update::Release, _: &str) -> Result<(), String> {
        Ok(())
    }
}

#[tokio::test]
async fn real_app_handler_respects_shared_update_exclusion_while_ping_survives() {
    use crate::app::AppState;
    use crate::update::{InstallWhen, UpdateConfig, UpdateService, UpdateState};

    let root = tempfile::tempdir().unwrap();
    let updates = Arc::new(
        UpdateService::new(
            UpdateConfig {
                status_path: root.path().join("update-status.json"),
                result_path: root.path().join("update-result.json"),
                running_version: "0.2.0".into(),
                platform: "linux-x86_64".into(),
                development_build: false,
                replaceable_development_build: false,
                running_from_cargo_target: false,
                check_interval: Duration::from_secs(86400),
            },
            Arc::new(UpdateBackend),
        )
        .unwrap(),
    );
    let state = Arc::new(Mutex::new(
        AppState::new_unrooted(root.path().join("worktrees"), "main", true, "unused.sock")
            .with_update_service(updates.clone()),
    ));
    let service = ExecutionService::new(AppState::handler(state));
    let session = open(&service);
    updates.check().await.unwrap();
    updates.install(InstallWhen::Idle, true).await.unwrap();
    assert_eq!(
        updates.tick(false).await.unwrap().state,
        UpdateState::Installing
    );
    let (output, _received, _gone) = sink();
    let mut mutation = submit(&service, &session, None, output, "user.present").await;
    let refused = reply(&mut mutation).await;
    assert_eq!(refused["ok"], false);
    assert_eq!(refused["error_code"], "busy");
    let (output, _received, _gone) = sink();
    let mut ping = submit(&service, &session, None, output, "ping").await;
    assert_eq!(reply(&mut ping).await["result"]["pong"], true);
    assert!(updates.admission().try_enter().is_none());
}
