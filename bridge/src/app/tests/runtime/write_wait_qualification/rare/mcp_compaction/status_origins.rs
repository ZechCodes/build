//! Real completion-status admission keeps the outstanding compaction original.
use super::*;
use crate::timing::write_wait::{IntervalCategory, WaitKind, WriteWaitRegistry};
use std::sync::atomic::{AtomicU64, Ordering};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn logical_write_compaction_completion_status_wait_retains_original_and_background() {
    let agent = CompactingAgent::without_background_services().with_context(200_000);
    {
        let mut app = agent.state.lock().unwrap();
        app.test_install_mcp_token(agent.agent_id.clone(), TOKEN.into());
    }
    let mut fixture = RareFixture::shared("compaction-status-admission", Arc::clone(&agent.state));
    let advanced = Arc::new(AtomicU64::new(0));
    let sampled = Arc::clone(&advanced);
    // Monotonic real intake plus a controlled offset; actual MCP arrival is
    // never compared with a frozen clock from before it was received.
    let registry = WriteWaitRegistry::with_clock(Arc::new(move || {
        Instant::now() + Duration::from_secs(sampled.load(Ordering::Acquire))
    }));
    fixture.rpc.registry = Arc::clone(&registry);
    let clock = Arc::clone(&agent.state.lock().unwrap().frame_clock);
    clock.set_write_wait_registry(Arc::clone(&registry));
    let (reported_tx, reported_rx) = std::sync::mpsc::channel();
    fixture._watchdog = registry.watchdog(
        Duration::from_millis(1),
        Arc::new(move |row| {
            let _ = reported_tx.send(row);
        }),
    );
    let key = TabKey::agent(&AppState::canonical_root(&agent.root), &agent.agent_id);
    let (session, instance) = {
        let app = agent.state.lock().unwrap();
        let tab = app.session_registry.test_tab(&key).unwrap();
        (
            Arc::clone(&tab.session),
            tab.session_instance.clone().unwrap(),
        )
    };
    let pump = MeasuredStatusPump::new(&agent.state, key, &session, instance);
    let pump = observe(&agent, pump, AgentStatus::Working).await;
    let proof = fixture
        .call_mcp(
            &agent.agent_id,
            TOKEN,
            BridgeAction::CompactSelf {
                instructions: "preserve the context-after evidence".into(),
            },
        )
        .await;
    // Wait for actual final root completion even on the baseline with no child.
    let _retained = fixture.origin_active(&proof).await;
    let pump = observe(&agent, pump, AgentStatus::Waiting).await;
    let sent = sent_compaction(&agent).await;
    if sent.is_none() {
        // A failed native-send prerequisite is a fixture failure, not a
        // missing-attribution RED. Close every real resource first.
        session.end();
        drop(pump);
        drop(fixture._watchdog);
        fixture.rpc.finish().await;
        panic!("actual /compact send and SentCompaction boundary were not reached");
    }
    {
        agent.state.lock().unwrap().record_agent_activity(
            RUN,
            &agent.agent_id,
            &AgentActivity::Compaction { completed: false },
            None,
        );
    }
    let mut completed = agent.status.borrow().clone();
    completed.context = Some(TurnContext {
        context_tokens: 17_000,
        cache_read_tokens: 0,
    });
    let retained_before = agent.state.lock().unwrap().read_view.load();
    let retained_board = retained_before.get_owned("board.list", "");
    let (arrived_tx, arrived_rx) = std::sync::mpsc::channel();
    clock.observe_lock_waits(Arc::new(move |method, _| {
        if method == "agent.poll_status" {
            let _ = arrived_tx.send(());
        }
    }));
    let state = Arc::clone(&agent.state);
    let (held_tx, held_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        let guard = state.lock().unwrap();
        let _ = held_tx.send(());
        let _ = release_rx.recv_timeout(Duration::from_secs(20));
        drop(guard);
    });
    let held = held_rx.recv_timeout(Duration::from_secs(5)).is_ok();
    let background_id = "qual/background/compaction-status/completion";
    let background = registry.begin_background(background_id, "agent.poll_status", Instant::now());
    let background_context = background.context();
    let worker = std::thread::spawn(move || {
        let mut pump = pump;
        let binding = background_context.bind_sync();
        pump.observe(&completed);
        drop(binding);
        background.complete();
        pump
    });
    let reached_actual_lock = arrived_rx.recv_timeout(Duration::from_secs(5)).is_ok();
    // The arrival callback precedes creation of the logical AppLock guard.
    // Poll only immutable registry roots; a correct implementation must not
    // lose because the worker had not published that guard at the pulse.
    let witness_deadline = Instant::now() + Duration::from_millis(300);
    let before = loop {
        let rows = registry.snapshot();
        let has_wait = |id: &str| {
            rows.iter()
                .find(|row| row.original_id == id)
                .is_some_and(|row| {
                    row.active.iter().any(|interval| {
                        interval.category == IntervalCategory::KnownWait(WaitKind::AppLock)
                    })
                })
        };
        if (has_wait(&proof.original_id) && has_wait(background_id))
            || Instant::now() >= witness_deadline
        {
            break rows;
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    advanced.store(6, Ordering::Release);
    let until = Instant::now() + Duration::from_millis(300);
    let mut original_report = None;
    while Instant::now() < until {
        if let Ok(row) = reported_rx.recv_timeout(Duration::from_millis(2)) {
            if row.original_id == proof.original_id {
                original_report = Some(row);
                break;
            }
        }
    }
    let during = registry.snapshot();
    let _ = release_tx.send(());
    let holder_joined = holder.join().is_ok();
    let worker_joined = worker.join();
    let row = {
        let mut app = agent.state.lock().unwrap();
        crate::app::tests::project_agent::items(&mut app, RUN, &agent.agent_id)
            .into_iter()
            .find(|row| row["data"]["event"] == "compaction")
    };
    session.end();
    let worker_ok = worker_joined.is_ok();
    drop(worker_joined);
    drop(fixture._watchdog);
    fixture.rpc.finish().await;
    // Every holder, actual callback and socket is cleaned before assertions.
    assert!(held && reached_actual_lock && holder_joined && worker_ok);
    assert_eq!(sent.unwrap(), ["/compact"]);
    assert_eq!(row.unwrap()["data"]["compaction"]["context_after"], 17_000);
    assert!(retained_board.is_some());
    assert_eq!(retained_before.get_owned("board.list", ""), retained_board);
    let original = before
        .iter()
        .find(|row| row.original_id == proof.original_id)
        .unwrap();
    let background = before
        .iter()
        .find(|row| row.original_id == background_id)
        .unwrap();
    let app_wait = IntervalCategory::KnownWait(WaitKind::AppLock);
    assert!(
        background
            .active
            .iter()
            .any(|interval| interval.category == app_wait),
        "{background:?}"
    );
    assert!(
        original.root_finished && !original.outstanding.is_empty(),
        "{original:?}"
    );
    assert!(
        original
            .active
            .iter()
            .any(|interval| interval.category == app_wait),
        "saved compaction original must own actual completion admission: {original:?}"
    );
    let unfinished = during
        .iter()
        .find(|row| row.original_id == proof.original_id)
        .unwrap();
    assert!(unfinished.breached(), "{unfinished:?}");
    assert_eq!(
        original_report.as_ref().map(|row| &row.original_id),
        Some(&proof.original_id)
    );
}
