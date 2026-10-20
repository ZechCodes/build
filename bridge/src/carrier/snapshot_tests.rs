use super::testing::{client_request, session_init};
use super::*;
use crate::test_support::off_lock::OffLockGate;
use crate::timing::FrameClock;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn snapshots_bypass_saturated_write_and_sql_admission() {
    let root = tempfile::tempdir().unwrap();
    let state = crate::app::AppState::new_unrooted(
        root.path().join("worktrees"),
        "main",
        true,
        "/tmp/saturation-snapshot.sock",
    )
    .shared();
    let snapshots = crate::app::AppState::handler(Arc::clone(&state));
    let (write_gate, write_held) = OffLockGate::new();
    let (sql_gate, sql_held) = OffLockGate::new();
    let handler = FrameHandler::new(FrameClock::new(), move |_, frame, _| {
        if frame.payload["method"] == "held" {
            write_gate.arrive();
        }
        json!({"id":frame.payload["id"],"ok":true})
    })
    .with_sql_reads(None, move |_, frame, _, _| {
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
    intake
        .open(
            "snapshots",
            &session_init("snapshots", intake.transport_public_key(), &key),
            &carrier,
        )
        .unwrap();
    outbound.try_recv().unwrap();
    send(&intake, &carrier, &key, 1, "held").await;
    write_held.wait_for_arrival();
    for id in 2..7 {
        send(&intake, &carrier, &key, id, "held").await;
    }
    for id in 10..18 {
        send(&intake, &carrier, &key, id, "tasks.get").await;
        sql_held.wait_for_arrival();
    }
    let app_held = Arc::new(AtomicBool::new(false));
    let held = Arc::clone(&app_held);
    let (entered, waiting) = std::sync::mpsc::channel();
    let (release_app, released) = std::sync::mpsc::channel();
    let app_holder = std::thread::spawn(move || {
        let _guard = state.lock().unwrap();
        held.store(true, Ordering::SeqCst);
        entered.send(()).unwrap();
        let _ = released.recv_timeout(Duration::from_secs(10));
        held.store(false, Ordering::SeqCst);
    });
    waiting.recv_timeout(Duration::from_secs(2)).unwrap();
    for (offset, method) in crate::api::effects::INTAKE_SNAPSHOT_METHODS
        .iter()
        .enumerate()
    {
        let id = 100 + offset;
        send(&intake, &carrier, &key, id, method).await;
        let reply = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let sealed = outbound.recv().await.expect("carrier open");
                let value = SessionSender::decrypt_push(&key, &sealed);
                if value["id"] == id && value.get("ok").is_some() {
                    break value;
                }
            }
        })
        .await
        .expect("snapshot bypasses all queues");
        assert!(
            app_held.load(Ordering::SeqCst),
            "{method} waited for the app mutex"
        );
        match *method {
            "workspace.get" | "agent.list" => {
                assert_eq!(reply["error_code"], "not_found", "{reply}")
            }
            "bridge.update_status" => assert_eq!(reply["error_code"], "unavailable", "{reply}"),
            _ => assert_eq!(reply["ok"], true, "{method}: {reply}"),
        }
    }
    release_app.send(()).unwrap();
    app_holder.join().unwrap();
    write_held.release();
    sql_held.release();
}

async fn send(intake: &FrameIntake, carrier: &CarrierHandle, key: &str, id: usize, method: &str) {
    let params = match method {
        "workspace.get" => json!({"workspace_id":"missing"}),
        "agent.list" => json!({"entity_id":"missing"}),
        _ => json!({}),
    };
    intake
        .accept(
            client_request(
                key,
                "snapshots",
                "data",
                json!({
                    "id":id,"method":method,"params":params
                }),
            ),
            carrier,
        )
        .await
        .unwrap();
}
