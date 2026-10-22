use super::testing::{client_request, session_init, within_patience};
use super::*;
use crate::timing::FrameClock;
use serde_json::{json, Value};
use std::sync::mpsc as std_mpsc;
use std::time::Duration;

const SESSION_ID: &str = "timing-session";
const PATIENCE: Duration = Duration::from_secs(5);

fn blocked_handler(
    clock: Arc<FrameClock>,
) -> (FrameHandler, std_mpsc::Receiver<()>, std_mpsc::Sender<()>) {
    let (arrived, running) = std_mpsc::channel();
    let (release, released) = std_mpsc::channel();
    let released = Mutex::new(released);
    let handler = FrameHandler::new(clock, move |_sender, frame, _timer| {
        if frame.payload["id"] == 1 {
            arrived
                .send(())
                .expect("the test is waiting for the worker");
            released
                .lock()
                .unwrap()
                .recv_timeout(PATIENCE)
                .expect("the test releases the worker");
        }
        json!({ "id": frame.payload["id"], "ok": true })
    });
    (handler, running, release)
}

fn opened_intake(
    handler: FrameHandler,
    pool_depth: usize,
) -> (
    Arc<FrameIntake>,
    CarrierHandle,
    mpsc::UnboundedReceiver<OutboundEnvelope>,
    String,
) {
    let transport = transport::generate_transport_keypair();
    let intake = FrameIntake::with_pool_and_admission(handler, transport, pool_depth, 1, 8);
    let (carrier, mut outbound) = CarrierHandle::open_channel();
    let key = transport::generate_session_key();
    let init = session_init(SESSION_ID, intake.transport_public_key(), &key);
    intake.open(SESSION_ID, &init, &carrier).unwrap();
    outbound.try_recv().expect("session accept was sent");
    (intake, carrier, outbound, key)
}

async fn request(intake: &FrameIntake, carrier: &CarrierHandle, key: &str, id: u64, method: &str) {
    intake
        .accept(
            client_request(
                key,
                SESSION_ID,
                "data",
                json!({ "id": id, "method": method }),
            ),
            carrier,
        )
        .await
        .expect("the frame was admitted");
}

async fn answers_through(
    outbound: &mut mpsc::UnboundedReceiver<OutboundEnvelope>,
    key: &str,
    last_id: u64,
) {
    let mut answered = std::collections::BTreeSet::new();
    while !answered.contains(&last_id) {
        let frame = SessionSender::decrypt_push(key, &within_patience(outbound.recv()).await);
        if frame.get("accepted").is_none() {
            answered.insert(frame["id"].as_u64().expect("an answer has an id"));
        }
    }
}

async fn wait_for_queue_depth(clock: &FrameClock, expected: u64) {
    tokio::time::timeout(PATIENCE, async {
        while clock.stats()["queue_depth"] != expected {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("the dispatcher reached the expected queue depth");
}

fn queue_stats(clock: &FrameClock, method: &str) -> Value {
    clock.stats()["methods"][method]["queued"].clone()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_frame_parked_in_wire_admission_keeps_its_intake_wait() {
    let clock = FrameClock::new();
    let (handler, running, release) = blocked_handler(clock.clone());
    let (intake, carrier, mut outbound, key) = opened_intake(handler, 1);

    request(&intake, &carrier, &key, 1, "capture.cancel").await;
    running
        .recv_timeout(PATIENCE)
        .expect("the only worker reached the gate");
    request(&intake, &carrier, &key, 2, "capture.cancel").await;
    request(&intake, &carrier, &key, 3, "capture.cancel").await;
    wait_for_queue_depth(&clock, 2).await;

    // The fourth frame has not reached Dispatcher::dispatch: one slot is full
    // and the admitter is waiting to place the third. Its wait must still be
    // charged when the worker eventually starts it.
    request(&intake, &carrier, &key, 4, "project.list").await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(clock.stats()["queue_depth"], 2);
    release.send(()).unwrap();
    answers_through(&mut outbound, &key, 4).await;

    let queued = queue_stats(&clock, "project.list");
    assert_eq!(queued["count"], 1, "{queued}");
    assert!(queued["sum_ms"].as_f64().unwrap() >= 30.0, "{queued}");
    assert_eq!(clock.stats()["methods"]["project.list"]["served"], 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fold_reports_the_oldest_intake_and_one_computation() {
    let clock = FrameClock::new();
    let (handler, running, release) = blocked_handler(clock.clone());
    let (intake, carrier, mut outbound, key) = opened_intake(handler, 2);

    request(&intake, &carrier, &key, 1, "capture.cancel").await;
    running
        .recv_timeout(PATIENCE)
        .expect("the only worker reached the gate");
    request(&intake, &carrier, &key, 2, "board.list").await;
    wait_for_queue_depth(&clock, 1).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    request(&intake, &carrier, &key, 3, "board.list").await;
    // Admission preserves this wire's order. Once the nonfolded sentinel has
    // its own ticket, request 3 must already have joined request 2's fold.
    request(&intake, &carrier, &key, 4, "capture.cancel").await;
    wait_for_queue_depth(&clock, 2).await;
    release.send(()).unwrap();
    answers_through(&mut outbound, &key, 4).await;

    let queued = queue_stats(&clock, "board.list");
    assert_eq!(queued["count"], 1, "{queued}");
    assert!(queued["sum_ms"].as_f64().unwrap() >= 30.0, "{queued}");
    assert_eq!(clock.stats()["methods"]["board.list"]["served"], 1);
}

#[test]
fn an_immediate_frame_counts_waiting_for_a_blocking_thread() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let (entered, began) = tokio::sync::oneshot::channel();
        let (release, released) = std_mpsc::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            let _ = entered.send(());
            released
                .recv_timeout(PATIENCE)
                .expect("the test releases the blocking thread");
        });
        tokio::time::timeout(PATIENCE, began)
            .await
            .expect("the blocking worker started")
            .unwrap();

        let clock = FrameClock::new();
        let handler = FrameHandler::new(
            clock.clone(),
            |_sender, frame, _timer| json!({ "id": frame.payload["id"], "ok": true }),
        );
        let (intake, carrier, mut outbound, key) = opened_intake(handler, 1);
        request(&intake, &carrier, &key, 1, "thread.attach").await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        release.send(()).unwrap();
        blocker.await.unwrap();
        answers_through(&mut outbound, &key, 1).await;

        let queued = queue_stats(&clock, "thread.attach");
        assert_eq!(queued["count"], 1, "{queued}");
        assert!(queued["sum_ms"].as_f64().unwrap() >= 30.0, "{queued}");
    });
}
