use super::*;
use crate::test_support::off_lock::OffLockGate;
use crate::update::{
    InstallWhen, Release, UpdateBackend, UpdateConfig, UpdateService, UpdateState,
};

struct ReleaseFixture;

#[async_trait::async_trait]
impl UpdateBackend for ReleaseFixture {
    async fn latest(&self) -> Result<Release, String> {
        Ok(Release {
            version: "1.1.0".into(),
            tag: "fixture".into(),
            published_at: None,
        })
    }

    async fn install(&self, _: &Release, _: &str) -> Result<(), String> {
        Ok(())
    }
}

async fn scheduled_update(root: &Path) -> Arc<UpdateService> {
    let updates = Arc::new(
        UpdateService::new(
            UpdateConfig {
                status_path: root.join("status.json"),
                result_path: root.join("result.json"),
                running_version: "1.0.0".into(),
                platform: "fixture".into(),
                development_build: false,
                replaceable_development_build: false,
                running_from_cargo_target: false,
                check_interval: Duration::from_secs(86_400),
            },
            Arc::new(ReleaseFixture),
        )
        .unwrap(),
    );
    updates.check().await.unwrap();
    assert_eq!(
        updates
            .install(InstallWhen::Idle, true)
            .await
            .unwrap()
            .state,
        UpdateState::ScheduledWhenIdle
    );
    updates
}

async fn reads_during_idle_probe(method: &'static str) {
    let directory = tempfile::tempdir().unwrap();
    let updates = scheduled_update(directory.path()).await;
    let (probe, held_probe) = OffLockGate::new();
    assert!(updates
        .set_working_agents_probe(Arc::new(move || {
            probe.arrive();
            true
        }))
        .is_ok());
    let mut reads = NativeReads::configured_with_updates(true, Some(Arc::clone(&updates)));
    let params = match method {
        "capture.get" => json!({"capture_id":reads.capture}),
        "task.get" | "task.stages" => json!({"task_id":reads.task}),
        "tasks.columns" => json!({"project_id":reads.project}),
        _ => json!({"stream_id":"stream-native-read"}),
    };
    let baseline = reads.held_read(method, params.clone()).await;
    assert_eq!(
        baseline["ok"], true,
        "unblocked real-service fixture: {baseline}"
    );
    match method {
        "capture.get" => assert_eq!(baseline["result"]["text"], "native capture"),
        "task.get" => assert_eq!(baseline["result"]["goal"], "native read goal"),
        "task.stages" => assert_eq!(baseline["result"]["stages"], json!([])),
        "tasks.columns" => assert!(!baseline["result"]["columns"].as_array().unwrap().is_empty()),
        _ => assert_eq!(baseline["result"]["complete"], true),
    }
    let ticker = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(updates.tick(false))
    });
    held_probe.wait_for_arrival();
    let started = Instant::now();
    // Snapshot intake is synchronous. Isolate a blocked request from the
    // controller that must release the real service's busy callback on red.
    let mut reading = tokio::spawn(async move { reads.held_read(method, params).await });
    let observed = tokio::time::timeout(Duration::from_millis(100), &mut reading).await;
    let elapsed = started.elapsed();
    drop(held_probe);
    let tick = ticker.join().unwrap().unwrap();
    let timely = observed.is_ok();
    let reply = match observed {
        Ok(reply) => reply,
        Err(_) => reading.await,
    };
    eprintln!(
        "{}",
        json!({"method":method,"busy_probe_held_through_reply":timely,
        "reply_us":elapsed.as_micros(),"deadline_us":100_000})
    );
    assert!(
        timely,
        "{method} waited for the update busy callback: {elapsed:?}"
    );
    let reply = reply.unwrap();
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(reply["result"], baseline["result"], "{method}");
    assert_eq!(tick.state, UpdateState::ScheduledWhenIdle);
}

macro_rules! idle_probe_read {
    ($test:ident, $method:literal) => {
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn $test() {
            reads_during_idle_probe($method).await;
        }
    };
}

idle_probe_read!(capture_get_succeeds_during_idle_probe, "capture.get");
idle_probe_read!(task_get_succeeds_during_idle_probe, "task.get");
idle_probe_read!(task_stages_succeeds_during_idle_probe, "task.stages");
idle_probe_read!(tasks_columns_succeeds_during_idle_probe, "tasks.columns");
idle_probe_read!(stream_events_succeeds_during_idle_probe, "stream.events");
idle_probe_read!(stream_state_succeeds_during_idle_probe, "stream.state");

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn active_sql_read_prevents_idle_probe_from_closing_admission() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let directory = tempfile::tempdir().unwrap();
    let updates = scheduled_update(directory.path()).await;
    let (probe, held_probe) = OffLockGate::new();
    let first = AtomicBool::new(true);
    assert!(updates
        .set_working_agents_probe(Arc::new(move || {
            if first.swap(false, Ordering::SeqCst) {
                probe.arrive();
            }
            false
        }))
        .is_ok());
    let reads = NativeReads::configured_with_updates(true, Some(Arc::clone(&updates)));
    let (service, pool) = {
        let app = reads.state.lock().unwrap();
        (app.read_service(), app.store.as_ref().unwrap().read_pool())
    };
    let mut connection = pool.acquire().await;
    let (query, held_query) = OffLockGate::new();
    let (entered, arrived) = mpsc::channel();
    connection.observe_transactions(move |_, _, _| {
        entered.send(()).unwrap();
        query.arrive();
    });
    let ticking = Arc::clone(&updates);
    let ticker = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(ticking.tick(false))
    });
    held_probe.wait_for_arrival();
    let task = reads.task.clone();
    let reader = std::thread::spawn(move || {
        service.call(
            req("thread.page", json!({"entity_id":task})),
            Some(connection),
        )
    });
    let entered_during_probe = arrived.recv_timeout(Duration::from_millis(100));
    drop(held_probe);
    let tick = ticker.join();
    // This observer runs after the SQL commit, before ReadService can release
    // its admission lease. Always let it finish before checking either result.
    drop(held_query);
    let reply = reader.join();
    assert!(
        entered_during_probe.is_ok(),
        "SQL read waited for the busy callback"
    );
    assert_eq!(tick.unwrap().unwrap().state, UpdateState::ScheduledWhenIdle);
    let reply = reply.unwrap();
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(
        updates.tick(false).await.unwrap().state,
        UpdateState::Installing
    );
}
