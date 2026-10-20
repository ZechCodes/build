use super::testing::{client_request, session_init};
use super::*;
use crate::test_support::off_lock::OffLockGate;
use crate::timing::FrameClock;
use std::time::Duration;

#[test]
fn a_late_frame_cannot_rebind_a_released_carrier_to_a_surviving_opening() {
    let registry = SessionRegistry::default();
    let (a, _a_out) = CarrierHandle::open_channel();
    let (b, _b_out) = CarrierHandle::open_channel();
    let key = transport::generate_session_key();
    registry.open("sql", key.clone(), &a).unwrap();
    registry.open("sql", key.clone(), &b).unwrap();
    assert!(registry.release_carrier(&a).is_empty());
    let frame = client_request(&key, "sql", "data", json!({"id":1,"method":"tasks.list"}));
    assert!(
        registry.admit(&frame, &a).is_err(),
        "release fences in-flight intake too"
    );
    assert!(registry.open("sql", key, &a).is_err());
    assert!(registry.admit(&frame, &b).is_ok());
}

struct Client {
    intake: Arc<FrameIntake>,
    carrier: CarrierHandle,
    rx: mpsc::UnboundedReceiver<OutboundEnvelope>,
    key: String,
}

impl Client {
    fn new(handler: FrameHandler) -> Self {
        let transport = transport::generate_transport_keypair();
        let intake = FrameIntake::with_pool(handler, transport, 1, 1);
        Self::attach(intake, transport::generate_session_key())
    }

    fn attach(intake: Arc<FrameIntake>, key: String) -> Self {
        let (carrier, mut rx) = CarrierHandle::open_channel();
        let init = session_init("sql", intake.transport_public_key(), &key);
        intake.open("sql", &init, &carrier).unwrap();
        rx.try_recv().unwrap();
        Self {
            intake,
            carrier,
            rx,
            key,
        }
    }

    async fn send(&self, id: usize, method: &str, params: Value) {
        self.intake
            .accept(
                client_request(
                    &self.key,
                    "sql",
                    "data",
                    json!({"id":id,"method":method,"params":params}),
                ),
                &self.carrier,
            )
            .await
            .unwrap();
    }

    async fn reply(&mut self) -> Value {
        tokio::time::timeout(Duration::from_secs(5), answer(&mut self.rx, &self.key))
            .await
            .expect("reply arrives")
    }
}

fn gated_handler(gate: OffLockGate) -> FrameHandler {
    FrameHandler::new(FrameClock::new(), move |_, frame, _| {
        if frame.payload["params"]["hold"] == true {
            gate.arrive();
        }
        json!({"id":frame.payload["id"],"ok":true})
    })
}

async fn fill_workers(client: &Client, held: &crate::test_support::off_lock::OffLockGateHandle) {
    for id in 0..sql::SQL_WORKERS {
        client
            .send(id, "tasks.get", json!({"hold":true,"id":id}))
            .await;
        held.wait_for_arrival();
    }
}

async fn answer(rx: &mut mpsc::UnboundedReceiver<OutboundEnvelope>, key: &str) -> Value {
    loop {
        let outbound = rx.recv().await.expect("carrier stays open");
        let payload = SessionSender::decrypt_push(key, &outbound);
        if payload.get("ok").is_some() {
            return payload;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sql_reads_use_dedicated_workers_even_with_a_term_id() {
    let (gate, held) = OffLockGate::new();
    let handler = FrameHandler::new(FrameClock::new(), move |_, frame, _| {
        if frame.payload["method"] == "held" {
            gate.arrive();
        }
        json!({"id":frame.payload["id"],"ok":true,
            "thread":std::thread::current().name()})
    });
    let transport = transport::generate_transport_keypair();
    let intake = FrameIntake::with_pool(handler, transport.clone(), 1, 1);
    let (carrier, mut rx) = CarrierHandle::open_channel();
    let key = transport::generate_session_key();
    intake
        .open(
            "sql",
            &session_init("sql", &transport.public_key_b64, &key),
            &carrier,
        )
        .unwrap();
    rx.try_recv().unwrap();
    intake
        .accept(
            client_request(&key, "sql", "data", json!({"id":1,"method":"held"})),
            &carrier,
        )
        .await
        .unwrap();
    held.wait_for_arrival();
    intake
        .accept(
            client_request(
                &key,
                "sql",
                "data",
                json!({"id":2,"method":"thread.page","params":{"term_id":"injected"}}),
            ),
            &carrier,
        )
        .await
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(1), answer(&mut rx, &key)).await;
    held.release();
    let result = result.expect("SQL does not wait for a write worker");
    assert_eq!(result["id"], 2);
    assert!(
        result["thread"].as_str().unwrap().starts_with("bridge-sql"),
        "{result}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sql_backlog_is_bounded_and_stats_bypasses_both_worker_pools() {
    let (gate, held) = OffLockGate::new();
    let client = Client::new(gated_handler(gate));
    client.send(90, "writer", json!({"hold":true})).await;
    held.wait_for_arrival();
    fill_workers(&client, &held).await;
    for id in 0..sql::SQL_BACKLOG {
        client.send(100 + id, "tasks.list", json!({"id":id})).await;
    }
    assert_eq!(client.intake.dispatcher.sql_depths(), (sql::SQL_BACKLOG, 1));
    let mut parked = {
        let intake = client.intake.clone();
        let carrier = client.carrier.clone();
        let envelope = client_request(
            &client.key,
            "sql",
            "data",
            json!({"id":200,"method":"tasks.get"}),
        );
        tokio::spawn(async move { intake.accept(envelope, &carrier).await })
    };
    assert!(tokio::time::timeout(Duration::from_millis(40), &mut parked)
        .await
        .is_err());
    assert_eq!(client.intake.dispatcher.sql_depths(), (sql::SQL_BACKLOG, 1));
    let mut other = Client::attach(client.intake.clone(), client.key.clone());
    other.send(201, "bridge.stats", json!({})).await;
    assert_eq!(other.reply().await["id"], 201);
    other.send(202, "ping", json!({})).await;
    assert_eq!(other.reply().await["id"], 202);
    client.intake.close_carrier(&client.carrier);
    tokio::time::timeout(Duration::from_secs(1), parked)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(client.intake.dispatcher.sql_depths(), (0, 0));
    // Dropping the gate releases all eight read workers and the writer.
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_a_keeps_b_queued_fold_and_discards_running_a_replies() {
    released_a_keeps_b(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn releasing_a_session_binding_preserves_b_and_prunes_a() {
    released_a_keeps_b(true).await;
}

async fn released_a_keeps_b(session_only: bool) {
    let (gate, held) = OffLockGate::new();
    let mut a = Client::new(gated_handler(gate));
    fill_workers(&a, &held).await;
    a.send(20, "tasks.list", json!({})).await;
    let mut b = Client::attach(a.intake.clone(), a.key.clone());
    b.send(21, "tasks.list", json!({})).await;
    assert_eq!(a.intake.dispatcher.sql_depths(), (2, 1));
    if session_only {
        a.intake.close_session("sql", &a.carrier);
    } else {
        a.intake.close_carrier(&a.carrier);
    }
    assert_eq!(a.intake.dispatcher.sql_depths(), (1, 1));
    held.release();
    assert_surviving_carrier_answers(&mut b).await;
    assert_only_receipts(&mut a);
    assert_reopened_session_answers(&a.intake, &b.carrier).await;
}

async fn assert_surviving_carrier_answers(b: &mut Client) {
    assert_eq!(b.reply().await["id"], 21);
    b.send(22, "thread.activity", json!({})).await;
    assert_eq!(b.reply().await["id"], 22);
}

fn assert_only_receipts(a: &mut Client) {
    // The original wire may retain receipts already sent, but never replies
    // computed after release. B is still the same open session.
    while let Ok(outbound) = a.rx.try_recv() {
        assert!(SessionSender::decrypt_push(&a.key, &outbound)
            .get("ok")
            .is_none());
    }
}

async fn assert_reopened_session_answers(intake: &Arc<FrameIntake>, carrier: &CarrierHandle) {
    intake.close_session("sql", carrier);
    assert_eq!(intake.dispatcher.sql_depths(), (0, 0));
    let mut reopened = Client::attach(intake.clone(), transport::generate_session_key());
    reopened.send(23, "tasks.get", json!({})).await;
    assert_eq!(reopened.reply().await["id"], 23);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn callers_waiting_for_connections_still_count_against_backlog() {
    let temporary = tempfile::tempdir().unwrap();
    let store = crate::store::Store::new(temporary.path()).unwrap();
    let pool = store.read_pool();
    let mut held = Vec::new();
    for _ in 0..sql::SQL_WORKERS {
        held.push(pool.acquire().await);
    }
    let handler = FrameHandler::new(FrameClock::new(), |_, _, _| panic!("SQL callback required"))
        .with_sql_reads(Some(pool), |_, frame, _, connection| {
            assert!(connection.is_some());
            json!({"id":frame.payload["id"],"ok":true})
        });
    let mut client = Client::new(handler);
    for id in 0..sql::SQL_BACKLOG {
        client.send(id, "tasks.get", json!({"id":id})).await;
    }
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(client.intake.dispatcher.sql_depths(), (sql::SQL_BACKLOG, 1));
    held.pop();
    for _ in 0..sql::SQL_BACKLOG {
        assert_eq!(client.reply().await["ok"], true);
    }
    assert_eq!(client.intake.dispatcher.sql_depths(), (0, 0));
}
