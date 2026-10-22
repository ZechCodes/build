use super::testing::{client_request, session_init};
use super::*;
use crate::test_support::off_lock::OffLockGate;
use crate::timing::FrameClock;
use std::future::Future;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn snapshots_bypass_saturated_write_and_sql_admission() {
    let root = tempfile::tempdir().unwrap();
    let state = snapshot_state(root.path());
    let store = state.lock().unwrap().attached_store_for_test().unwrap();
    let snapshots = crate::app::AppState::handler(Arc::clone(&state));
    let controls = snapshots.clone();
    let (write_gate, write_held) = OffLockGate::new();
    let (sql_gate, sql_held) = OffLockGate::new();
    let handler = FrameHandler::new(FrameClock::new(), move |sender, frame, _| {
        if frame.payload["method"] == "held" {
            write_gate.arrive();
            json!({"id":frame.payload["id"],"ok":true})
        } else {
            controls.call(sender, frame)
        }
    })
    .with_sql_reads(Some(store.read_pool()), move |_, frame, _, _connection| {
        sql_gate.arrive();
        json!({"id":frame.payload["id"],"ok":true})
    })
    .with_snapshot_reads(move |frame| {
        snapshots.call(SessionSender::detached("actual-snapshot"), frame)
    });
    let intake = FrameIntake::with_pool_and_admission(
        handler,
        transport::generate_transport_keypair(),
        1,
        1,
        1,
    );
    let (carrier, mut outbound) = CarrierHandle::open_channel();
    let key = transport::generate_session_key();
    open(&intake, &carrier, &key);
    outbound.try_recv().unwrap();
    send(&intake, &carrier, &key, 1, "held").await;
    write_held.wait_for_arrival();
    for id in 2..7 {
        send(&intake, &carrier, &key, id, "held").await;
    }
    for offset in 0..sql::SQL_WORKERS {
        send(&intake, &carrier, &key, 10 + offset, "tasks.get").await;
        sql_held.wait_for_arrival();
    }
    for offset in 0..sql::SQL_BACKLOG {
        send(&intake, &carrier, &key, 30 + offset, "tasks.get").await;
    }
    assert_eq!(
        intake.dispatcher.sql_depths(),
        (sql::SQL_BACKLOG, 1),
        "the fixture must fill the SQL backlog after occupying every worker"
    );
    let envelope = client_request(
        &key,
        "snapshots",
        "data",
        json!({"id":90,"method":"tasks.get","params":{"parked":true}}),
    );
    let mut parked = Box::pin(intake.accept(envelope, &carrier));
    let pending =
        std::future::poll_fn(|cx| std::task::Poll::Ready(parked.as_mut().poll(cx).is_pending()))
            .await;
    assert!(pending, "the next SQL request must park ordered intake");
    assert_eq!(intake.dispatcher.sql_depths(), (sql::SQL_BACKLOG, 1));

    let (probe, mut replies) = CarrierHandle::open_channel();
    open(&intake, &probe, &key);
    replies.try_recv().unwrap();
    assert_eq!(
        intake.registry.minted.load(Ordering::Relaxed),
        1,
        "the second carrier shares the saturated session opening"
    );
    let app_held = Arc::new(AtomicBool::new(false));
    let held = Arc::clone(&app_held);
    let (entered, waiting) = std::sync::mpsc::channel();
    let (release_app, released) = std::sync::mpsc::channel();
    let app_holder = std::thread::spawn(move || {
        let _guard = state.lock().unwrap();
        let mut writer = store.writer_for_test();
        let _transaction = writer
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        held.store(true, Ordering::SeqCst);
        entered.send(()).unwrap();
        let _ = released.recv_timeout(Duration::from_secs(10));
        held.store(false, Ordering::SeqCst);
    });
    waiting.recv_timeout(Duration::from_secs(2)).unwrap();
    probe_snapshots(&intake, &probe, &key, &mut replies, &app_held).await;
    probe_controls(&intake, &probe, &key, &mut replies, &app_held).await;
    assert_eq!(intake.dispatcher.sql_depths(), (sql::SQL_BACKLOG, 1));
    assert!(
        app_held.load(Ordering::SeqCst),
        "probes waited for a held writer"
    );

    let close = client_request(&key, "snapshots", transport::CLOSE_FRAME_TYPE, json!({}));
    tokio::time::timeout(Duration::from_secs(2), intake.accept(close, &probe))
        .await
        .expect("close bypasses saturated intake")
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), parked)
        .await
        .expect("session close releases its parked SQL intake")
        .unwrap();
    assert_eq!(intake.dispatcher.sql_depths(), (0, 0));
    assert!(
        app_held.load(Ordering::SeqCst),
        "close waited for a held writer"
    );
    release_app.send(()).unwrap();
    app_holder.join().unwrap();
    write_held.release();
    sql_held.release();
}

fn snapshot_state(root: &std::path::Path) -> Arc<Mutex<crate::app::AppState>> {
    let context =
        crate::harness::HarnessContext::resolved(root.join("unused.sock"), root.to_owned())
            .unwrap();
    crate::app::AppState::new_unrooted_configured(root.join("worktrees"), "main", true, context)
        .with_task_store(root.join("records"))
        .unwrap()
        .shared()
}

fn open(intake: &FrameIntake, carrier: &CarrierHandle, key: &str) {
    intake
        .open(
            "snapshots",
            &session_init("snapshots", intake.transport_public_key(), key),
            carrier,
        )
        .unwrap();
}

async fn probe_snapshots(
    intake: &FrameIntake,
    carrier: &CarrierHandle,
    key: &str,
    replies: &mut mpsc::UnboundedReceiver<OutboundEnvelope>,
    held: &AtomicBool,
) {
    assert_eq!(crate::api::effects::INTAKE_SNAPSHOT_METHODS.len(), 15);
    for (offset, method) in crate::api::effects::INTAKE_SNAPSHOT_METHODS
        .iter()
        .enumerate()
    {
        let id = 100 + offset;
        send(intake, carrier, key, id, method).await;
        let reply = reply(replies, key, id).await;
        assert!(
            held.load(Ordering::SeqCst),
            "{method} waited for the app or Store writer mutex"
        );
        assert_ne!(reply["error_code"], "busy", "{method}: {reply}");
        match *method {
            "workspace.get" | "agent.list" | "capture.get" | "task.get" | "task.stages"
            | "tasks.columns" | "stream.events" | "stream.state" => {
                assert_eq!(reply["error_code"], "not_found", "{reply}")
            }
            "bridge.update_status" => assert_eq!(reply["error_code"], "unavailable", "{reply}"),
            _ => assert_eq!(reply["ok"], true, "{method}: {reply}"),
        }
    }
}

async fn probe_controls(
    intake: &FrameIntake,
    carrier: &CarrierHandle,
    key: &str,
    replies: &mut mpsc::UnboundedReceiver<OutboundEnvelope>,
    held: &AtomicBool,
) {
    for (offset, method) in ["ping", "bridge.stats", "rtc.offer", "rtc.ice", "rtc.close"]
        .iter()
        .enumerate()
    {
        let id = 200 + offset;
        send(intake, carrier, key, id, method).await;
        let reply = reply(replies, key, id).await;
        assert!(
            held.load(Ordering::SeqCst),
            "{method} waited for a held writer"
        );
        assert_ne!(reply["error_code"], "busy", "{method}: {reply}");
        match *method {
            "ping" => assert_eq!(reply["result"]["pong"], true, "{reply}"),
            "bridge.stats" => assert!(reply["result"]["methods"].is_object(), "{reply}"),
            "rtc.close" => assert_eq!(
                reply["error"], "no peer connection for session snapshots",
                "{reply}"
            ),
            "rtc.offer" => assert_eq!(reply["error"], "missing required param: sdp", "{reply}"),
            _ => assert_eq!(
                reply["error"], "missing required param: candidate",
                "{reply}"
            ),
        }
    }
}

async fn reply(
    replies: &mut mpsc::UnboundedReceiver<OutboundEnvelope>,
    key: &str,
    id: usize,
) -> Value {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let sealed = replies.recv().await.expect("carrier open");
            let value = SessionSender::decrypt_push(key, &sealed);
            if value["id"] == id && value.get("ok").is_some() {
                break value;
            }
        }
    })
    .await
    .expect("snapshot or control bypasses all queues")
}

async fn send(intake: &FrameIntake, carrier: &CarrierHandle, key: &str, id: usize, method: &str) {
    let params = match method {
        "workspace.get" => json!({"workspace_id":"missing"}),
        "agent.list" => json!({"entity_id":"missing"}),
        "capture.get" => json!({"capture_id":"missing"}),
        "task.get" | "task.stages" => json!({"task_id":"missing"}),
        "tasks.columns" => json!({"project_id":"missing"}),
        "stream.events" | "stream.state" => json!({"stream_id":"missing"}),
        _ => json!({}),
    };
    intake
        .accept(
            client_request(
                key,
                "snapshots",
                "data",
                json!({
                    "id":id,"method":method,"params":params,
                }),
            ),
            carrier,
        )
        .await
        .unwrap();
}
