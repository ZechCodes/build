//! Real pre-reply waits must be visible before any completed frame sample.
use super::*;
use crate::app::AppState;
use crate::timing::write_wait::{WriteSnapshot, WAIT_BUDGET};

const FAULT: Duration = Duration::from_secs(6);
const SAFETY: Duration = Duration::from_secs(20);

struct HeldApp {
    release: Option<std_mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl HeldApp {
    fn take(state: Arc<Mutex<AppState>>) -> Self {
        let (held, ready) = std_mpsc::channel();
        let (release, released) = std_mpsc::channel();
        let thread = std::thread::spawn(move || {
            let _guard = state.lock().unwrap();
            let _ = held.send(());
            // No assertion while holding the application mutex: even a test
            // safety timeout must release it without poisoning it.
            let _ = released.recv_timeout(SAFETY);
        });
        let mut holder = Self {
            release: Some(release),
            thread: Some(thread),
        };
        if ready.recv_timeout(SAFETY).is_err() {
            holder.release();
            panic!("application lock holder did not start");
        }
        holder
    }

    fn release(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

impl Drop for HeldApp {
    fn drop(&mut self) {
        self.release();
    }
}

async fn final_reply(outbound: &mut mpsc::UnboundedReceiver<OutboundEnvelope>, key: &str) -> Value {
    loop {
        let envelope = tokio::time::timeout(SAFETY, outbound.recv())
            .await
            .expect("released RPC must finish")
            .expect("carrier remains open");
        let reply = SessionSender::decrypt_push(key, &envelope);
        if reply["id"] == 72201 && reply.get("accepted").is_none() {
            return reply;
        }
    }
}

fn assert_active_fault(row: Option<&WriteSnapshot>, fired: bool) {
    let row = row.expect("active RPC write must be registered before its app-lock wait");
    assert!(
        row.original_id.contains("72201"),
        "original caller identity: {row:?}"
    );
    assert!(!row.root_finished, "fault is observed before final reply");
    assert!(!row.outstanding.is_empty());
    assert!(row.blocked_or_sql_union > WAIT_BUDGET, "{row:?}");
    assert!(
        row.breached(),
        "unfinished write must breach without a completed histogram"
    );
    assert!(fired, "independent watchdog must observe the live breach");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn logical_write_rpc_six_second_app_wait_is_visible_while_active() {
    let directory = tempfile::tempdir().unwrap();
    let clock = FrameClock::new();
    let mut app = AppState::new_unrooted(directory.path(), "main", true, "unused");
    app.frame_clock = Arc::clone(&clock);
    let state = Arc::new(Mutex::new(app));
    let handler = AppState::handler(Arc::clone(&state));
    let registry = clock.capture_write_waits();
    let fired = Arc::new(Mutex::new(Vec::new()));
    let observe = Arc::clone(&fired);
    let watchdog = registry.watchdog(
        Duration::from_millis(20),
        Arc::new(move |row| observe.lock().unwrap().push(row)),
    );
    let (waiting, mut wait_started) = mpsc::unbounded_channel();
    clock.observe_requests(Arc::new(move |frame, phase, _| {
        if frame.payload["id"] == 72201 && phase == "app_wait_start" {
            let _ = waiting.send(());
        }
    }));
    let (intake, carrier, mut outbound, key) = opened_intake(handler, 2);
    let mut held = HeldApp::take(state);
    request(&intake, &carrier, &key, 72201, "settings.set").await;
    let reached_wait = tokio::time::timeout(PATIENCE, wait_started.recv()).await;
    tokio::time::sleep(FAULT).await;
    let active = registry.active_snapshot();
    let live = active.iter().find(|row| row.method == "settings.set");
    let live_fired = fired
        .lock()
        .unwrap()
        .iter()
        .any(|row| row.method == "settings.set");
    held.release();
    let reply = final_reply(&mut outbound, &key).await;
    drop(watchdog);

    assert!(
        reached_wait.is_ok_and(|value| value.is_some()),
        "real app wait was entered"
    );
    assert_eq!(
        reply["ok"], true,
        "fault request finishes after release: {reply}"
    );
    assert_active_fault(live, live_fired);
}
