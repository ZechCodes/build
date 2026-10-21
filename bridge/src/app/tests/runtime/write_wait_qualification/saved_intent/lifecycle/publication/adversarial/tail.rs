use super::*;

const A: &str = "qual/task-tail/retry";
const B: &str = "qual/task-tail/cancel";

struct HeldSubscriptions {
    release: Option<mpsc::Sender<()>>,
    worker: Option<std::thread::JoinHandle<bool>>,
}
impl HeldSubscriptions {
    fn start(bus: Arc<crate::changes::ChangeBus>) -> (Self, bool) {
        let (ready, entered) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            bus.with_subscription_lock_for_test(|| {
                let _ = ready.send(());
                released.recv_timeout(SAFETY).is_ok()
            })
        });
        let arrived = entered.recv_timeout(SAFETY).is_ok();
        (
            Self {
                release: Some(release),
                worker: Some(worker),
            },
            arrived,
        )
    }
    fn finish(&mut self) -> bool {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
        self.worker
            .take()
            .is_none_or(|worker| worker.join().unwrap_or(false))
    }
}
impl Drop for HeldSubscriptions {
    fn drop(&mut self) {
        self.finish();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn accepted_board_still_owes_actual_task_note_tail() {
    let fixture = Fixture::new();
    fixture.add_publication_peer(PEER);
    let (registry, offset) = clock();
    let rpc = RpcHarness::new(Arc::clone(&fixture.state), Arc::clone(&registry));
    let first = retry(&rpc, &fixture, A).await;
    let after_ack = row(&registry, A, false).await;
    let (bus, view) = {
        let app = fixture.state.lock().unwrap();
        (Arc::clone(&app.changes), app.read_view.clone())
    };
    let (mut held, arrived) = HeldSubscriptions::start(bus);
    let task_id = fixture.intent.task_id.clone();
    let (report, received) = mpsc::channel();
    let watchdog = registry.watchdog(
        Duration::from_millis(5),
        Arc::new(move |row| {
            if row.original_id == A {
                let _ = report.send(row);
            }
        }),
    );
    let cancellation = rpc.request(
        0,
        B,
        "tasks.assign",
        json!({"task_id":task_id,"assignee":null}),
    );
    let inspect = async {
        let deadline = Instant::now() + Duration::from_secs(2);
        let reached = loop {
            let accepted = board_task(&view.load(), &task_id).is_some_and(|task| {
                task["assignee"].is_null() && task["deferred_assignment"].is_null()
            });
            let waiting = snapshot(&registry, B).is_some_and(|row| {
                row.active.iter().any(|span| {
                    span.category == IntervalCategory::KnownWait(WaitKind::ResourceLock)
                })
            });
            if accepted && waiting {
                break true;
            }
            if Instant::now() >= deadline {
                break false;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        };
        let sql = rpc
            .request(
                1,
                "qual/task-tail/sql",
                "tasks.get",
                json!({"task_id":task_id}),
            )
            .await
            .reply;
        if reached {
            offset.fetch_add(6, Ordering::SeqCst);
        }
        let pending = snapshot(&registry, A);
        let observed = received.recv_timeout(Duration::from_secs(2)).ok();
        let released = held.finish();
        (reached, sql, pending, observed, released)
    };
    let (cancelled, (reached, sql, pending, observed, released)) =
        tokio::join!(cancellation, inspect);
    drop(watchdog);
    let terminal = row(&registry, A, true).await;
    let service = step(&fixture);
    let effects = finish(fixture, rpc).await;

    eprintln!("task_publication_adversarial={{\"case\":\"actual_note_tail\",\"qualification_successes\":0}}");
    assert_no_delivery(&effects);
    assert!(
        arrived && reached && released,
        "actual accepted-board/note-tail boundary was not reached"
    );
    for reply in [&first, &cancelled.reply, &sql] {
        assert_eq!(reply["ok"], true, "{reply}");
    }
    assert_eq!(service, Some(Ok(false)));
    assert!(sql["result"]["task"]["assignee"].is_null());
    assert_pending(&after_ack, A);
    assert_pending(&pending, A);
    let pending = pending.unwrap();
    assert!(pending
        .active
        .iter()
        .any(|span| span.category == IntervalCategory::KnownWait(WaitKind::ResourceLock)));
    assert!(!pending.active.iter().any(|span| matches!(
        span.category,
        IntervalCategory::KnownWait(WaitKind::Publication | WaitKind::Dispatch)
    )));
    let observed = observed.expect("independent watchdog sees the owed note-tail leaf wait");
    assert_eq!(observed.id, pending.id);
    assert!(observed.breached());
    let terminal = terminal.unwrap();
    assert_eq!(terminal.id, pending.id);
    assert!(terminal.completed_at.is_some() && terminal.root_finished && terminal.cancelled);
    assert!(terminal.outstanding.is_empty() && terminal.active.is_empty());
    assert!(!terminal.abandoned && terminal.breached());
}
