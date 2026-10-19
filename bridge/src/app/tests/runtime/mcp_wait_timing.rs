use super::*;
use crate::timing::{recording_clock, SLOW_FRAME};

const WAIT: Duration = Duration::from_millis(250);
const WATCHDOG: Duration = Duration::from_secs(5);

type Lines = Arc<Mutex<Vec<String>>>;

fn fixture() -> (
    tempfile::TempDir,
    Arc<Mutex<AppState>>,
    Arc<ControlPlane>,
    Lines,
) {
    let dir = tempfile::tempdir().unwrap();
    let (clock, lines) = recording_clock();
    let mut app = AppState::new_unrooted(dir.path(), "main", true, "unused");
    app.frame_clock = Arc::clone(&clock);
    (dir, app.shared(), ControlPlane::new(clock), lines)
}

fn duration(line: &str, field: &str) -> f64 {
    line.split_whitespace()
        .find_map(|part| part.strip_prefix(field))
        .and_then(|value| value.strip_suffix("ms"))
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| panic!("missing {field} in {line}"))
}

fn measured_wait(lines: &Lines, field: &str) {
    let lines = lines.lock().unwrap();
    assert_eq!(lines.len(), 1, "one completed slow frame: {lines:?}");
    let line = &lines[0];
    assert!(line.starts_with("slow frame mcp.control "), "{line}");
    assert!(
        duration(line, field) >= SLOW_FRAME.as_secs_f64() * 1000.0,
        "the coordinated wait is charged to {field}: {line}"
    );
}

#[tokio::test]
async fn mcp_initial_semaphore_wait_is_reported() {
    let (_dir, state, plane, lines) = fixture();
    let permits = Arc::clone(&plane.turns)
        .acquire_many_owned(CONTROL_FRAMES_AT_THE_LOCK as u32)
        .await
        .unwrap();
    let answer = plane.answer(state, json!({}));
    tokio::pin!(answer);
    assert!(futures_util::poll!(answer.as_mut()).is_pending());
    tokio::time::sleep(WAIT).await;
    drop(permits);
    assert!(answer.await.unwrap()["error"]
        .as_str()
        .unwrap()
        .contains("unauthorized"));
    measured_wait(&lines, "permit_wait=");
}

#[tokio::test]
async fn mcp_reacquiring_a_turn_reports_only_the_permit_wait() {
    let (_dir, _state, plane, lines) = fixture();
    let timer = plane.clock.frame(MCP_CONTROL_METHOD);
    let mut turn = Turn::take(&plane).await;
    let (reserved, permits) = tokio::sync::oneshot::channel();
    let work = async {
        tokio::time::sleep(WAIT).await;
        let held = Arc::clone(&plane.turns)
            .acquire_many_owned(CONTROL_FRAMES_AT_THE_LOCK as u32)
            .await
            .unwrap();
        reserved.send(held).unwrap();
    };
    let aside = turn.aside(work);
    tokio::pin!(aside);
    tokio::select! {
        () = &mut aside => panic!("the reacquisition is blocked by the test"),
        held = permits => {
            let held = held.unwrap();
            tokio::time::sleep(WAIT).await;
            drop(held);
        }
    }
    aside.await;
    drop(timer);
    measured_wait(&lines, "permit_wait=");
    let lines = lines.lock().unwrap();
    assert!(
        duration(&lines[0], "total=") - duration(&lines[0], "permit_wait=") >= 200.0,
        "the deferred work itself is not permit wait: {lines:?}"
    );
}

#[test]
fn mcp_blocking_pool_wait_is_reported() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    let (_dir, state, plane, lines) = fixture();
    runtime.block_on(async {
        let (started, running) = tokio::sync::oneshot::channel();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let blocker = tokio::task::spawn_blocking(move || {
            started.send(()).unwrap();
            released.recv_timeout(WATCHDOG)
        });
        running.await.unwrap();
        let answer = plane.answer(state, json!({}));
        tokio::pin!(answer);
        assert!(futures_util::poll!(answer.as_mut()).is_pending());
        tokio::time::sleep(WAIT).await;
        release.send(()).unwrap();
        assert!(answer.await.is_some());
        blocker.await.unwrap().unwrap();
    });
    measured_wait(&lines, "pool_wait=");
}

#[tokio::test]
async fn mcp_admission_lock_wait_is_reported() {
    let (_dir, state, plane, lines) = fixture();
    let (held, is_held) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel::<()>();
    let locked = Arc::clone(&state);
    let holder = std::thread::spawn(move || {
        let _guard = locked.lock().unwrap();
        held.send(()).unwrap();
        released.recv_timeout(WATCHDOG)
    });
    is_held.recv_timeout(WATCHDOG).unwrap();
    let (_observation, mut sections) =
        crate::app::runtime::off_the_workers::observe_sections_for_test();
    let answer = plane.answer(state, json!({}));
    tokio::pin!(answer);
    assert!(futures_util::poll!(answer.as_mut()).is_pending());
    tokio::time::timeout(WATCHDOG, sections.recv())
        .await
        .unwrap()
        .unwrap();
    tokio::time::sleep(WAIT).await;
    release.send(()).unwrap();
    assert!(answer.await.is_some());
    holder.join().unwrap().unwrap();
    measured_wait(&lines, "lock_wait=");
}

#[test]
fn mcp_deferred_pool_wait_excludes_job_execution() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    let (_dir, state, plane, lines) = fixture();
    let job = {
        let mut app = state.lock().unwrap();
        app.deferred_work = Some(crate::app::DeferredWork::External(Box::new(|| {
            std::thread::sleep(WAIT);
            Ok(json!({ "finished": true }))
        })));
        app.take_deferred().unwrap()
    };
    runtime.block_on(async {
        let timer = plane.clock.frame(MCP_CONTROL_METHOD);
        let mut turn = Turn::take(&plane).await;
        let (started, running) = tokio::sync::oneshot::channel();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let blocker = tokio::task::spawn_blocking(move || {
            started.send(()).unwrap();
            released.recv_timeout(WATCHDOG)
        });
        running.await.unwrap();
        let applying = apply_off_the_socket(&state, &timer, &mut turn, job);
        tokio::pin!(applying);
        assert!(futures_util::poll!(applying.as_mut()).is_pending());
        tokio::time::sleep(WAIT).await;
        release.send(()).unwrap();
        assert_eq!(applying.await, Ok(json!({ "finished": true })));
        blocker.await.unwrap().unwrap();
    });
    measured_wait(&lines, "pool_wait=");
    let lines = lines.lock().unwrap();
    assert!(
        duration(&lines[0], "total=") - duration(&lines[0], "pool_wait=") >= 200.0,
        "the job itself is not blocking pool wait: {lines:?}"
    );
}

#[test]
fn rpc_admission_lock_wait_is_reported() {
    let (_dir, state, _plane, lines) = fixture();
    let handler = AppState::handler(Arc::clone(&state));
    let guard = state.lock().unwrap();
    let (started, running) = std::sync::mpsc::channel();
    let answering = std::thread::spawn(move || {
        started.send(()).unwrap();
        handler.call(
            crate::carrier::SessionSender::detached("admission-timing"),
            crate::transport::Frame {
                session_id: "admission-timing".into(),
                message_id: "timed-request".into(),
                frame_type: "data".into(),
                sender: "client".into(),
                created_at: "test".into(),
                payload: json!({ "id": 1, "method": "board.list", "params": {} }),
            },
        )
    });
    running.recv_timeout(WATCHDOG).unwrap();
    std::thread::sleep(WAIT);
    drop(guard);
    answering.join().unwrap();
    let lines = lines.lock().unwrap();
    assert_eq!(lines.len(), 1, "one completed RPC: {lines:?}");
    assert!(lines[0].starts_with("slow frame board.list "), "{lines:?}");
    assert!(
        duration(&lines[0], "lock_wait=") >= SLOW_FRAME.as_secs_f64() * 1000.0,
        "RPC admission is charged to lock_wait: {lines:?}"
    );
}
