//! Every task the daemon runs on its own takes the app mutex somewhere, and
//! the app mutex is a blocking lock. A tick that waited for it on a runtime
//! worker would park that worker behind whatever frame holds it — and with
//! every worker parked, the I/O driver stops: data channels stall, terminals
//! stop painting, `bridge.stats` cannot answer. So each of them waits for it on
//! the blocking pool, and these prove it on the smallest runtime there is: ONE
//! worker, a holder keeping the lock for 300 ms from a thread of its own, the
//! task's tick falling due inside that hold, and a 10 ms nap on the worker
//! that still wakes in time.
use super::*;
use crate::app::runtime::deferred::{spawn_off_lock, OffLockJob};

/// How long the holder keeps the app mutex: a slow frame's hold.
const HOLD: Duration = Duration::from_millis(300);
/// What the worker is asked to do meanwhile.
const NAP: Duration = Duration::from_millis(10);
/// A nap that took this long was a worker parked on the lock.
const LATE: Duration = Duration::from_millis(100);
/// The period of the timer-driven tasks under test: several ticks fall due
/// inside one hold.
const TICK: Duration = Duration::from_millis(20);

/// Hold the app mutex from a thread of its own for [`HOLD`], call `due` once
/// it is held — whatever makes the task under test want the lock — and nap on
/// the runtime's worker until the holder lets go. Asserts no nap was late.
async fn the_worker_naps_on_time_while_the_lock_is_held(
    state: &Arc<Mutex<AppState>>,
    due: impl FnOnce(),
) {
    let (held, is_held) = tokio::sync::oneshot::channel();
    let holder = {
        let state = Arc::clone(state);
        std::thread::spawn(move || {
            let _guard = state.lock().unwrap();
            let _ = held.send(());
            std::thread::sleep(HOLD);
        })
    };
    is_held.await.expect("the holder took the app mutex");
    due();
    // Timed from the test's own thread, which is no worker: the task under
    // test may be scheduled ahead of anything spawned now, and a nap started
    // after it had parked would time nothing. The nap's timer is fired by the
    // runtime's worker, so a worker parked on the lock still makes it late.
    let mut longest = Duration::ZERO;
    while !holder.is_finished() {
        let started = std::time::Instant::now();
        tokio::time::sleep(NAP).await;
        longest = longest.max(started.elapsed());
    }
    holder.join().expect("the holder let go");
    assert!(
        longest < LATE,
        "a {NAP:?} nap took {longest:?} while another thread held the app mutex: \
         the task under test parked the runtime's only worker on it"
    );
}

fn unrooted_state(dir: &Path) -> Arc<Mutex<AppState>> {
    AppState::new_unrooted(dir, "main", true, "unused").shared()
}

fn instance_of(entity_id: &str) -> SessionInstance {
    SessionInstance {
        id: format!("session-{entity_id}"),
        entity_id: entity_id.to_string(),
        agent_id: crate::agent::derived_agent_id(entity_id),
        conversation_id: crate::agent::derived_agent_id(entity_id),
        checkout: "/nowhere".to_string(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn the_status_pump_waits_for_the_app_mutex_off_the_workers() {
    let dir = tempfile::tempdir().unwrap();
    let state = unrooted_state(dir.path());
    let (_status, watched) = tokio::sync::watch::channel(
        crate::harness::SessionStatusSnapshot::new(AgentStatus::Waiting),
    );
    let session: Arc<dyn AgentSession> =
        Arc::new(DictatedSession::reporting(AgentStatus::Waiting).watching_status(watched));

    the_worker_naps_on_time_while_the_lock_is_held(&state, || {
        crate::app::spawn_status_pump(
            &state,
            derived_agent_key(dir.path(), "run-status"),
            Arc::clone(&session),
            Some(instance_of("run-status")),
            session.status_changed(),
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn the_receipt_pump_waits_for_the_app_mutex_off_the_workers() {
    let dir = tempfile::tempdir().unwrap();
    let state = unrooted_state(dir.path());
    let (_receipts, watched) = tokio::sync::watch::channel(crate::harness::TurnReceiptSnapshot {
        seen_operation_ids: vec!["op-1".into()],
        uncertain_operation_ids: Vec::new(),
    });
    let session: Arc<dyn AgentSession> =
        Arc::new(DictatedSession::reporting(AgentStatus::Working).watching_receipts(watched));

    the_worker_naps_on_time_while_the_lock_is_held(&state, || {
        crate::app::runtime::delivery::receipts::spawn_receipt_pump(
            &state,
            &session,
            Some(instance_of("run-receipts")),
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn the_activity_pump_waits_for_the_app_mutex_off_the_workers() {
    let dir = tempfile::tempdir().unwrap();
    let state = unrooted_state(dir.path());
    let session: Arc<dyn AgentSession> = Arc::new(DictatedSession::reporting(AgentStatus::Working));
    let (activity, subscribed) = broadcast::channel(16);
    crate::app::spawn_activity_pump(
        &state,
        derived_agent_key(dir.path(), "run-activity"),
        Arc::clone(&session),
        Some(instance_of("run-activity")),
        Some(subscribed),
        None,
    );

    the_worker_naps_on_time_while_the_lock_is_held(&state, || {
        let _ = activity.send(crate::harness::ActivityReport::own_work(
            crate::harness::AgentActivity::Reasoning {
                summary: "while the lock is held".into(),
            },
        ));
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn a_byte_pumps_death_rites_wait_for_the_app_mutex_off_the_workers() {
    let dir = tempfile::tempdir().unwrap();
    let state = unrooted_state(dir.path());
    let session: Arc<dyn AgentSession> = Arc::new(DictatedSession::reporting(AgentStatus::Working));
    let (bytes, subscribed) = broadcast::channel::<Vec<u8>>(16);
    crate::app::runtime::pumps::spawn_tab_pump(
        &state,
        derived_agent_key(dir.path(), "run-bytes"),
        session,
        Some(instance_of("run-bytes")),
        crate::app::runtime::pumps::BytePump {
            screen: Some(ScreenHandle::new("agent:run-bytes", 80, 24)),
            rx: Some(subscribed),
            push_runtime: None,
        },
    );

    the_worker_naps_on_time_while_the_lock_is_held(&state, move || drop(bytes)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn the_idle_monitor_waits_for_the_app_mutex_off_the_workers() {
    let dir = tempfile::tempdir().unwrap();
    let state = unrooted_state(dir.path());
    AppState::spawn_idle_monitor(Arc::clone(&state), Duration::from_secs(600), TICK);

    the_worker_naps_on_time_while_the_lock_is_held(&state, || {}).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn the_terminal_reaper_waits_for_the_app_mutex_off_the_workers() {
    let dir = tempfile::tempdir().unwrap();
    let state = unrooted_state(dir.path());
    AppState::spawn_terminal_reaper(Arc::clone(&state), TICK);

    the_worker_naps_on_time_while_the_lock_is_held(&state, || {}).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn the_update_check_waits_for_the_app_mutex_off_the_workers() {
    let dir = tempfile::tempdir().unwrap();
    let state = unrooted_state(dir.path());
    AppState::spawn_update_checks(Arc::clone(&state), TICK);

    the_worker_naps_on_time_while_the_lock_is_held(&state, || {}).await;
}

/// One agent's end of the control socket, the daemon's end served the way
/// `serve_done_listener` serves it.
#[cfg(unix)]
fn an_agent_on(
    state: &Arc<Mutex<AppState>>,
    plane: &Arc<crate::app::mcp::ControlPlane>,
) -> std::os::unix::net::UnixStream {
    let (agent, daemon) = std::os::unix::net::UnixStream::pair().expect("a socket pair");
    daemon
        .set_nonblocking(true)
        .expect("the daemon's end is async");
    tokio::spawn(crate::app::mcp::handle_done_stream(
        Arc::clone(state),
        tokio::net::UnixStream::from_std(daemon).expect("the daemon's end joins the runtime"),
        Arc::clone(plane),
    ));
    agent
}

#[cfg(unix)]
fn a_tool_call(agent: &mut std::os::unix::net::UnixStream) {
    use std::io::Write;
    let frame = json!({
        "task_id": "run-tool",
        "session_token": "not-the-token",
        "request": { "action": "read_unread_messages" },
    });
    agent
        .write_all(format!("{frame}\n").as_bytes())
        .expect("the frame is written");
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn an_agents_tool_call_waits_for_the_app_mutex_off_the_workers() {
    let dir = tempfile::tempdir().unwrap();
    let state = unrooted_state(dir.path());
    let plane = crate::app::mcp::ControlPlane::new(Arc::clone(&state.lock().unwrap().frame_clock));
    let mut agent = an_agent_on(&state, &plane);

    the_worker_naps_on_time_while_the_lock_is_held(&state, || a_tool_call(&mut agent)).await;
}

/// Off the workers, every agent's tool call could wait at the app mutex at
/// once — and a frame the user is waiting on would then queue behind all of
/// them. The agents take turns instead: a dozen calls made while the lock is
/// held are all answered, parking no worker, with no more than
/// [`CONTROL_FRAMES_AT_THE_LOCK`] of them at the lock at a time.
///
/// [`CONTROL_FRAMES_AT_THE_LOCK`]: crate::app::mcp::CONTROL_FRAMES_AT_THE_LOCK
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn a_dozen_agents_tool_calls_take_turns_at_the_app_mutex() {
    use std::io::BufRead;

    let dir = tempfile::tempdir().unwrap();
    let state = unrooted_state(dir.path());
    let plane = crate::app::mcp::ControlPlane::new(Arc::clone(&state.lock().unwrap().frame_clock));
    let mut agents: Vec<_> = (0..12).map(|_| an_agent_on(&state, &plane)).collect();

    the_worker_naps_on_time_while_the_lock_is_held(&state, || {
        agents.iter_mut().for_each(a_tool_call);
    })
    .await;
    let answered = tokio::task::spawn_blocking(move || {
        agents
            .into_iter()
            .map(|agent| {
                agent
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .expect("a read timeout");
                let mut answer = String::new();
                std::io::BufReader::new(agent)
                    .read_line(&mut answer)
                    .expect("the agent is answered");
                answer
            })
            .filter(|answer| answer.contains("unauthorized"))
            .count()
    })
    .await
    .expect("the answers were read");

    assert_eq!(answered, 12, "every agent's call is answered");
    let most = plane.most_at_the_lock();
    assert!(
        (1..=crate::app::mcp::CONTROL_FRAMES_AT_THE_LOCK).contains(&most),
        "{most} control frames were at the app mutex at once"
    );
}

/// A frame's turn at the app mutex is given back while the git it handed
/// back runs with the guard released: two agents' slow workspace operations
/// in flight at once leave the lock free and every turn free, and a third
/// agent's tool call is answered in the meantime (#131 review). Each of the
/// two takes a turn again to write its outcome down.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn a_slow_workspace_operation_gives_its_turn_back_while_its_git_runs() {
    use crate::app::mcp::{apply_off_the_socket, ControlPlane, Turn};
    use std::io::BufRead;

    let dir = tempfile::tempdir().unwrap();
    let state = unrooted_state(dir.path());
    let clock = Arc::clone(&state.lock().unwrap().frame_clock);
    let plane = ControlPlane::new(Arc::clone(&clock));
    let (started, running) = std::sync::mpsc::channel();
    let mut finishes = Vec::new();
    let mut slow = Vec::new();
    for _ in 0..crate::app::mcp::CONTROL_FRAMES_AT_THE_LOCK {
        let (finish, finished) = std::sync::mpsc::channel::<()>();
        finishes.push(finish);
        let started = started.clone();
        let job = {
            let mut app = state.lock().unwrap();
            app.deferred_work = Some(crate::app::DeferredWork::External(Box::new(move || {
                started.send(()).unwrap();
                finished.recv().unwrap();
                Ok(json!({ "slow": true }))
            })));
            app.take_deferred().expect("the operation was deferred")
        };
        let (state, plane, clock) = (Arc::clone(&state), Arc::clone(&plane), Arc::clone(&clock));
        slow.push(tokio::spawn(async move {
            let timer = clock.frame(crate::app::mcp::MCP_CONTROL_METHOD);
            let mut turn = Turn::take(&plane).await;
            apply_off_the_socket(&state, &timer, &mut turn, job).await
        }));
    }
    let running = tokio::task::spawn_blocking(move || {
        for _ in 0..crate::app::mcp::CONTROL_FRAMES_AT_THE_LOCK {
            running
                .recv_timeout(Duration::from_secs(5))
                .expect("the slow operation started");
        }
    });
    running.await.unwrap();

    let mut agent = an_agent_on(&state, &plane);
    a_tool_call(&mut agent);
    let answer = tokio::task::spawn_blocking(move || {
        agent
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("a read timeout");
        let mut answer = String::new();
        std::io::BufReader::new(agent)
            .read_line(&mut answer)
            .map(|_| answer)
    })
    .await
    .unwrap();
    assert!(
        answer
            .as_deref()
            .is_ok_and(|answer| answer.contains("unauthorized")),
        "the third agent was answered while two operations ran: {answer:?}"
    );

    finishes
        .into_iter()
        .for_each(|finish| finish.send(()).unwrap());
    for operation in slow {
        let written = tokio::time::timeout(Duration::from_secs(5), operation)
            .await
            .expect("each operation takes a turn again and writes its outcome down")
            .unwrap();
        assert_eq!(written, Ok(json!({ "slow": true })));
    }
    assert!(plane.most_at_the_lock() <= crate::app::mcp::CONTROL_FRAMES_AT_THE_LOCK);
}

/// A job whose decide phase is immediate, so its apply phase falls due inside
/// the hold.
struct Immediate;

impl OffLockJob for Immediate {
    type Claim = ();
    type Decided = ();
    fn claim(&mut self) {}
    fn decide(self) {}
    fn apply(_state: &mut AppState, _claim: (), _decided: ()) {}
    fn abandon(_state: &mut AppState, _claim: ()) {}
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn an_off_lock_jobs_apply_phase_waits_for_the_app_mutex_off_the_workers() {
    let dir = tempfile::tempdir().unwrap();
    let state = unrooted_state(dir.path());

    the_worker_naps_on_time_while_the_lock_is_held(&state, || {
        assert!(
            spawn_off_lock(Arc::clone(&state), Immediate).is_ok(),
            "a runtime is under the test"
        );
    })
    .await;
}

/// What the bridge pushes to its clients runs on the push runtime, never on
/// the main one: the change bus's flusher from the moment the state is
/// shared, and each terminal's byte pump from the moment its tab opens — the
/// vt100 parse and the frame per client every 10 ms (#131).
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn the_flusher_and_a_terminals_paint_run_on_the_push_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let push = crate::liveness::DedicatedRuntime::push().unwrap();
    let pushing = || push.handle().metrics().num_alive_tasks();
    let main = tokio::runtime::Handle::current();
    let state = AppState::new_unrooted(dir.path(), "main", true, "unused")
        .with_push_runtime(push.handle())
        .shared();
    assert_eq!(pushing(), 1, "the flusher is on the push runtime");
    let on_main = main.metrics().num_alive_tasks();

    let key = derived_agent_key(dir.path(), "run-paint");
    let (tab, output) = Tab::spawn_shell(
        &HarnessSpec::new("cat"),
        key.tab_id.clone(),
        dir.path().to_path_buf(),
        terminal_size(80, 24),
    )
    .expect("the tab spawns");
    let session = Arc::clone(&tab.session);
    let pumps = state
        .lock()
        .unwrap()
        .session_registry
        .insert_shell(key.clone(), tab, output);
    crate::app::spawn_tab_pumps(&state, key, pumps);

    assert_eq!(
        pushing(),
        2,
        "the terminal's byte pump is on the push runtime"
    );
    assert_eq!(
        main.metrics().num_alive_tasks(),
        on_main,
        "and nothing of it is on the main runtime"
    );
    session.end();
    push.stop();
}
