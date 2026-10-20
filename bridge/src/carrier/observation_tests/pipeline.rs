use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_full_sql_blocked_opening_does_not_delay_another_openings_pure_read() {
    let root = tempfile::tempdir().unwrap();
    let store = crate::store::Store::new(root.path()).unwrap();
    let pool = store.read_pool();
    let mut held = Vec::new();
    for _ in 0..sql::SQL_WORKERS {
        held.push(pool.acquire().await);
    }
    let handler = FrameHandler::new(FrameClock::new(), |_, _, _| panic!("reads bypass app"))
        .with_observation_reads(Some(pool), |frame, connection| {
            drop(connection);
            Box::new(move || json!({"id":frame.payload["id"],"ok":true}))
        });
    let a = Client::new(handler);
    let mut b = Client::on_intake(a.intake.clone(), "independent");
    for id in 0..sql::SQL_BACKLOG {
        a.send(id, "tasks.review.get", json!({"unique":id})).await;
    }
    assert_eq!(
        a.intake.dispatcher.observation_depths(),
        (sql::SQL_BACKLOG, 1)
    );
    let parked = a.send(99, "tasks.review.get", json!({"unique":99}));
    tokio::pin!(parked);
    let pending = std::future::poll_fn(|cx| {
        use std::future::Future;
        std::task::Poll::Ready(parked.as_mut().poll(cx).is_pending())
    })
    .await;
    assert!(
        pending,
        "request seventeen is bounded by its opening's admission slots"
    );
    b.send(100, "fs.list", json!({})).await;
    let answer = tokio::time::timeout(Duration::from_secs(2), b.reply()).await;
    drop(held);
    parked.await;
    assert_eq!(
        answer.expect("the independent opening's pure read bypasses SQL backlog")["id"],
        100
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ready_metadata_releases_sql_resources_while_all_physical_io_workers_are_held() {
    let root = tempfile::tempdir().unwrap();
    let store = crate::store::Store::new(root.path()).unwrap();
    let pool = store.read_pool();
    let (gate, held) = OffLockGate::new();
    let prepared = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = prepared.clone();
    let handler = FrameHandler::new(FrameClock::new(), |_, _, _| panic!("reads bypass app"))
        .with_sql_reads(Some(pool.clone()), |_, frame, _, connection| {
            assert!(connection.is_some());
            json!({"id":frame.payload["id"],"ok":true})
        })
        .with_observation_reads(Some(pool.clone()), move |frame, connection| {
            if frame.payload["method"] == "fs.list" {
                assert!(connection.is_none());
                let gate = gate.clone();
                return Box::new(move || {
                    gate.arrive();
                    json!({"id":frame.payload["id"],"ok":true})
                });
            }
            assert!(connection.is_some());
            drop(connection);
            count.fetch_add(1, Ordering::SeqCst);
            Box::new(move || json!({"id":frame.payload["id"],"ok":true}))
        });
    let starts = handler.observation_worker_starts();
    let mut client = Client::new(handler);
    for id in 0..sql::IO_WORKERS {
        client.send(id, "fs.list", json!({"unique":id})).await;
        held.wait_for_arrival();
    }
    for id in 20..20 + sql::SQL_WORKERS {
        client
            .send(id, "tasks.review.get", json!({"unique":id}))
            .await;
    }
    let reached = tokio::time::timeout(Duration::from_secs(5), async {
        while prepared.load(Ordering::SeqCst) != sql::SQL_WORKERS {
            tokio::task::yield_now().await;
        }
    })
    .await;
    // Ready work consumes bounded admission slots, with no ninth physical worker.
    let depth = client.intake.dispatcher.observation_depths();
    let mut connections = Vec::new();
    for _ in 0..sql::SQL_WORKERS {
        connections.push(tokio::time::timeout(Duration::from_secs(2), pool.acquire()).await);
    }
    let connections_released = connections.iter().all(Result::is_ok);
    drop(connections);
    client.send(100, "tasks.get", json!({})).await;
    let sql_reply = tokio::time::timeout(Duration::from_secs(2), client.reply()).await;
    drop(held);
    reached.expect("metadata prepares despite all external I/O workers being held");
    assert_eq!(depth, (sql::SQL_WORKERS, 1));
    assert!(
        connections_released,
        "Preparing/Ready owns no SQLite connection after metadata"
    );
    assert_eq!(
        sql_reply.expect("Ready work does not hold shared SQL execution permits")["id"],
        100
    );
    assert_eq!(starts.load(Ordering::SeqCst), sql::IO_WORKERS);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mixed_metadata_makes_progress_with_continuous_pure_and_ordinary_sql_traffic() {
    let root = tempfile::tempdir().unwrap();
    let store = crate::store::Store::new(root.path()).unwrap();
    let (gate, held) = OffLockGate::new();
    let prepared = Arc::new(tokio::sync::Notify::new());
    let progress = prepared.clone();
    let handler = FrameHandler::new(FrameClock::new(), |_, _, _| panic!("reads bypass app"))
        .with_sql_reads(None, move |_, frame, _, connection| {
            assert!(connection.is_none());
            if frame.payload["params"]["hold"] == true {
                gate.arrive();
            }
            json!({"id":frame.payload["id"],"ok":true})
        })
        .with_observation_reads(Some(store.read_pool()), move |frame, connection| {
            if connection.is_some() {
                drop(connection);
                progress.notify_one();
            }
            Box::new(move || json!({"id":frame.payload["id"],"ok":true}))
        });
    let a = Client::new(handler);
    let mut b = Client::on_intake(a.intake.clone(), "pure-traffic");
    for id in 0..sql::SQL_WORKERS {
        a.send(id, "tasks.get", json!({"hold":true,"unique":id}))
            .await;
        held.wait_for_arrival();
    }
    a.send(50, "tasks.review.get", json!({})).await;
    // Real replies keep the runtime servicing both lanes while the SQL waiter
    // establishes its fair place. No sleep or SQL-resource reservation in I/O.
    for id in 100..116 {
        b.send(id, "fs.list", json!({})).await;
        assert_eq!(b.reply().await["id"], id);
    }
    for id in 200..200 + sql::SQL_BACKLOG {
        a.send(id, "tasks.get", json!({"unique":id})).await;
    }
    let traffic = tokio::spawn(async move {
        for id in 300..364 {
            b.send(id, "fs.list", json!({})).await;
            assert_eq!(b.reply().await["id"], id);
        }
    });
    held.release();
    let mixed_progress = tokio::time::timeout(Duration::from_secs(5), prepared.notified()).await;
    drop(held);
    mixed_progress
        .expect("mixed metadata retains its fair SQL waiter despite pure and SQL traffic");
    traffic.await.unwrap();
}
