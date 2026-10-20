use super::testing::{client_request, session_init};
use super::*;
use crate::test_support::off_lock::OffLockGate;
use crate::timing::FrameClock;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn observation_reads_bypass_the_held_write_worker() {
    let (gate, held) = OffLockGate::new();
    let handler = FrameHandler::new(FrameClock::new(), move |_, frame, _| {
        if frame.payload["method"] == "held" {
            gate.arrive();
        }
        json!({"id":frame.payload["id"],"ok":true})
    })
    .with_observation_reads(None, |frame, _connection| {
        Box::new(move || {
            json!({"id":frame.payload["id"],"ok":true,
            "worker":std::thread::current().name()})
        })
    });
    let transport = transport::generate_transport_keypair();
    let intake = FrameIntake::with_pool(handler, transport, 1, 1);
    let (carrier, mut rx) = CarrierHandle::open_channel();
    let key = transport::generate_session_key();
    intake
        .open(
            "io",
            &session_init("io", intake.transport_public_key(), &key),
            &carrier,
        )
        .unwrap();
    rx.try_recv().unwrap();
    intake
        .accept(
            client_request(&key, "io", "data", json!({"id":1,"method":"held"})),
            &carrier,
        )
        .await
        .unwrap();
    held.wait_for_arrival();
    intake
        .accept(
            client_request(&key, "io", "data", json!({"id":2,"method":"fs.list"})),
            &carrier,
        )
        .await
        .unwrap();
    let reply = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let envelope = rx.recv().await.unwrap();
            let value = SessionSender::decrypt_push(&key, &envelope);
            if value.get("ok").is_some() {
                break value;
            }
        }
    })
    .await;
    held.release();
    let reply = reply.expect("observation completes while the writer is held");
    assert_eq!(reply["id"], 2);
    assert!(reply["worker"].as_str().unwrap().starts_with("bridge-io-"));
}

struct Client {
    intake: Arc<FrameIntake>,
    carrier: CarrierHandle,
    rx: mpsc::UnboundedReceiver<OutboundEnvelope>,
    key: String,
}

impl Client {
    fn new(handler: FrameHandler) -> Self {
        let intake = FrameIntake::with_pool(handler, transport::generate_transport_keypair(), 1, 1);
        let (carrier, mut rx) = CarrierHandle::open_channel();
        let key = transport::generate_session_key();
        intake
            .open(
                "observations",
                &session_init("observations", intake.transport_public_key(), &key),
                &carrier,
            )
            .unwrap();
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
                    "observations",
                    "data",
                    json!({"id":id,"method":method,"params":params}),
                ),
                &self.carrier,
            )
            .await
            .unwrap();
    }

    async fn reply(&mut self) -> Value {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let envelope = self.rx.recv().await.unwrap();
                let value = SessionSender::decrypt_push(&self.key, &envelope);
                if value.get("ok").is_some() {
                    break value;
                }
            }
        })
        .await
        .expect("observation reply arrives")
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mixed_observation_prepares_on_sql_and_releases_all_connections_before_io() {
    let root = tempfile::tempdir().unwrap();
    let store = crate::store::Store::new(root.path()).unwrap();
    let pool = store.read_pool();
    let (gate, held) = OffLockGate::new();
    let handler = FrameHandler::new(FrameClock::new(), |_, _, _| {
        panic!("app dispatch is forbidden")
    })
    .with_sql_reads(Some(pool.clone()), |_, frame, _, connection| {
        assert!(connection.is_some());
        json!({"id":frame.payload["id"],"ok":true})
    })
    .with_observation_reads(Some(pool.clone()), move |frame, connection| {
        assert!(std::thread::current()
            .name()
            .unwrap()
            .starts_with("bridge-sql-"));
        assert!(connection.is_some());
        drop(connection);
        let gate = gate.clone();
        Box::new(move || {
            assert!(std::thread::current()
                .name()
                .unwrap()
                .starts_with("bridge-io-"));
            gate.arrive();
            json!({"id":frame.payload["id"],"ok":true})
        })
    });
    let handler = handler.with_snapshot_reads(|frame| json!({"id":frame.payload["id"],"ok":true}));
    let mut client = Client::new(handler);
    for id in 0..sql::IO_WORKERS {
        client
            .send(id, "tasks.review.get", json!({"unique":id}))
            .await;
        held.wait_for_arrival();
    }
    // Hold every SQLite connection at once while every I/O worker is gated.
    let mut connections = Vec::new();
    for _ in 0..sql::SQL_WORKERS {
        connections.push(
            tokio::time::timeout(Duration::from_secs(2), pool.acquire())
                .await
                .expect("I/O work must release its metadata connection"),
        );
    }
    drop(connections);
    client.send(99, "tasks.get", json!({})).await;
    let answer = client.reply().await;
    assert_eq!(
        answer["id"], 99,
        "SQL reads run while all I/O workers are held"
    );
    client.send(100, "board.list", json!({})).await;
    assert_eq!(client.reply().await["id"], 100);
    client.send(101, "ping", json!({})).await;
    assert_eq!(client.reply().await["id"], 101);
    for _ in 0..sql::IO_WORKERS {
        held.release();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pure_observation_does_not_wait_for_sql_pool_capacity() {
    let root = tempfile::tempdir().unwrap();
    let store = crate::store::Store::new(root.path()).unwrap();
    let pool = store.read_pool();
    let mut held = Vec::new();
    for _ in 0..sql::SQL_WORKERS {
        held.push(pool.acquire().await);
    }
    let handler = FrameHandler::new(FrameClock::new(), |_, _, _| {
        panic!("app dispatch is forbidden")
    })
    .with_observation_reads(Some(pool), |frame, connection| {
        assert!(connection.is_none());
        Box::new(move || json!({"id":frame.payload["id"],"ok":true}))
    });
    let mut client = Client::new(handler);
    client.send(1, "fs.list", json!({})).await;
    assert_eq!(client.reply().await["id"], 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn observations_fold_only_queued_callers() {
    let (gate, held) = OffLockGate::new();
    let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let calls = count.clone();
    let handler = FrameHandler::new(FrameClock::new(), |_, _, _| {
        panic!("app dispatch is forbidden")
    })
    .with_observation_reads(None, move |frame, _| {
        let gate = gate.clone();
        let calls = calls.clone();
        Box::new(move || {
            calls.fetch_add(1, Ordering::SeqCst);
            if frame.payload["params"]["hold"] == true {
                gate.arrive();
            }
            json!({"id":frame.payload["id"],"ok":true})
        })
    });
    let mut client = Client::new(handler);
    for id in 0..sql::IO_WORKERS {
        client
            .send(id, "fs.list", json!({"hold":true,"unique":id}))
            .await;
        held.wait_for_arrival();
    }
    client.send(20, "git.status", json!({})).await;
    client.send(21, "git.status", json!({})).await;
    // A running read must not absorb a later identical read.
    client
        .send(22, "fs.list", json!({"hold":true,"unique":0}))
        .await;
    held.release();
    held.wait_for_arrival();
    for _ in 0..sql::IO_WORKERS {
        held.release();
    }
    let mut answered = std::collections::BTreeSet::new();
    for _ in 0..sql::IO_WORKERS + 3 {
        answered.insert(client.reply().await["id"].as_u64().unwrap());
    }
    assert!(answered.contains(&20) && answered.contains(&21) && answered.contains(&22));
    assert_eq!(count.load(Ordering::SeqCst), sql::IO_WORKERS + 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_mixed_preparation_waits_releases_io_slots_before_sql_connections() {
    let root = tempfile::tempdir().unwrap();
    let store = crate::store::Store::new(root.path()).unwrap();
    let pool = store.read_pool();
    let mut connections = Vec::new();
    for _ in 0..sql::SQL_WORKERS {
        connections.push(pool.acquire().await);
    }
    let handler = FrameHandler::new(
        FrameClock::new(),
        |_, frame, _| json!({"id":frame.payload["id"],"ok":true}),
    )
    .with_observation_reads(Some(pool), |frame, connection| {
        assert!(
            connection.is_none(),
            "closed metadata requests never execute"
        );
        Box::new(move || json!({"id":frame.payload["id"],"ok":true}))
    });
    let mut client = Client::new(handler);
    for id in 0..sql::IO_WORKERS {
        client
            .send(id, "tasks.review.get", json!({"unique":id}))
            .await;
    }
    tokio::time::timeout(Duration::from_secs(2), async {
        while client.intake.dispatcher.observation_depths() != (0, 0) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("all metadata reads wait for pool checkout");
    client.intake.close_carrier(&client.carrier);
    let (carrier, mut rx) = CarrierHandle::open_channel();
    client
        .intake
        .open(
            "observations",
            &session_init(
                "observations",
                client.intake.transport_public_key(),
                &client.key,
            ),
            &carrier,
        )
        .unwrap();
    rx.try_recv().unwrap();
    client.carrier = carrier;
    client.rx = rx;
    client.send(99, "fs.list", json!({})).await;
    assert_eq!(client.reply().await["id"], 99);
    assert_eq!(
        connections.len(),
        sql::SQL_WORKERS,
        "pool remains exhausted until after the probe"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn observation_handler_timing_ends_before_sealing_its_reply() {
    let clock = FrameClock::new();
    let (recorded, values) = std::sync::mpsc::channel();
    let observed_clock = Arc::downgrade(&clock);
    clock.observe_requests(Arc::new(move |_frame, phase, _at| {
        if phase == "reply_start" {
            let stats = observed_clock.upgrade().unwrap().stats();
            recorded
                .send(stats["methods"]["fs.list"]["served"].as_u64().unwrap_or(0))
                .unwrap();
        }
    }));
    let handler = FrameHandler::new(clock, |_, _, _| panic!("observation bypasses app"))
        .with_observation_reads(None, |frame, _| {
            Box::new(move || json!({"id":frame.payload["id"],"ok":true}))
        });
    let mut client = Client::new(handler);
    client.send(1, "fs.list", json!({})).await;
    assert_eq!(client.reply().await["id"], 1);
    assert_eq!(values.recv_timeout(Duration::from_secs(1)).unwrap(), 1,
        "FrameClock must use the legacy handler-complete endpoint; reply sealing is a separate measured phase");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cold_pure_observation_does_not_initialize_sql_execution_threads() {
    let handler = FrameHandler::new(FrameClock::new(), |_, _, _| {
        panic!("pure observation bypasses app")
    })
    .with_observation_reads(None, |frame, _| {
        Box::new(move || json!({"id":frame.payload["id"],"ok":true}))
    });
    let sql_execution = handler.read_executors.clone();
    assert!(sql_execution.get().is_none());
    let mut client = Client::new(handler);
    client.send(1, "fs.list", json!({})).await;
    assert_eq!(client.reply().await["id"], 1);
    assert!(
        sql_execution.get().is_none(),
        "a first pure read must not pay for SQL threads it never uses"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn io_threads_grow_only_for_concurrent_work_and_reuse_warm_workers() {
    let (gate, held) = OffLockGate::new();
    let handler = FrameHandler::new(FrameClock::new(), |_, _, _| {
        panic!("observation bypasses app")
    })
    .with_observation_reads(None, move |frame, _| {
        let gate = gate.clone();
        Box::new(move || {
            if frame.payload["params"]["hold"] == true {
                gate.arrive();
            }
            json!({"id":frame.payload["id"],"ok":true})
        })
    });
    let starts = handler.io_thread_starts.clone();
    let mut client = Client::new(handler);
    assert_eq!(starts.load(Ordering::SeqCst), 0);
    // Count the entire first serial batch, so deferred startups cannot hide
    // behind a better first request or a benchmark warmup.
    for id in 0..22 {
        client.send(id, "fs.list", json!({"serial":id})).await;
        assert_eq!(client.reply().await["id"], id);
        assert_eq!(
            starts.load(Ordering::SeqCst),
            1,
            "serial reads reuse the first I/O worker"
        );
    }
    for id in 100..100 + sql::IO_WORKERS {
        client
            .send(id, "fs.list", json!({"hold":true,"unique":id}))
            .await;
        held.wait_for_arrival();
    }
    assert_eq!(
        starts.load(Ordering::SeqCst),
        sql::IO_WORKERS,
        "all eight gated jobs have workers"
    );
    client
        .send(108, "fs.list", json!({"hold":true,"unique":108}))
        .await;
    assert_eq!(
        client.intake.dispatcher.observation_depths(),
        (1, 1),
        "the ninth job stays queued"
    );
    assert_eq!(
        starts.load(Ordering::SeqCst),
        sql::IO_WORKERS,
        "the worker limit is eight"
    );
    held.release();
    held.wait_for_arrival();
    assert_eq!(
        starts.load(Ordering::SeqCst),
        sql::IO_WORKERS,
        "the queued job reuses a released worker"
    );
    for _ in 0..sql::IO_WORKERS {
        held.release();
    }
    let mut answered = std::collections::BTreeSet::new();
    for _ in 0..sql::IO_WORKERS + 1 {
        answered.insert(client.reply().await["id"].as_u64().unwrap());
    }
    assert_eq!(answered, (100..=108).collect());
    assert_eq!(starts.load(Ordering::SeqCst), sql::IO_WORKERS);
}
