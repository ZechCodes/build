use super::*;
use std::sync::mpsc;

pub(super) const READ_BUDGET: Duration = Duration::from_millis(100);

/// Own the holder before waiting for readiness, so unwinding always releases it.
struct HeldApp {
    release: Option<mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl HeldApp {
    fn new(state: Arc<Mutex<AppState>>) -> Self {
        let (ready, waiting) = mpsc::sync_channel(1);
        let (release, released) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let _guard = state.lock().unwrap();
            let _ = ready.send(());
            let _ = released.recv();
        });
        let held = Self {
            release: Some(release),
            thread: Some(thread),
        };
        waiting
            .recv_timeout(WATCHDOG)
            .expect("the dedicated holder never acquired AppState");
        held
    }
}

impl Drop for HeldApp {
    fn drop(&mut self) {
        self.release.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub(super) struct Request {
    pub(super) frame: Value,
    pub(super) expected: Value,
    label: String,
}

impl Request {
    pub(super) fn new(frame: Value, expected: Value) -> Self {
        Self {
            label: frame["request"]["action"].as_str().unwrap().into(),
            frame,
            expected,
        }
    }
}

struct Measured {
    request: Request,
    reading: tokio::task::JoinHandle<Value>,
    early: Option<Result<Value, tokio::task::JoinError>>,
    elapsed: Duration,
}

pub(super) async fn group(
    fixture: &Fixture,
    requests: Vec<Request>,
    release_capacity: impl FnOnce(),
) {
    let turns = fixture.held_turns().await;
    let turn_count = turns.len();
    let holder = HeldApp::new(fixture.state.clone());
    let mut measured = Vec::new();
    for request in requests {
        let started = std::time::Instant::now();
        let mut reading = tokio::spawn(socket_reply(
            fixture.state.clone(),
            fixture.plane.clone(),
            request.frame.clone(),
        ));
        let early = tokio::time::timeout(READ_BUDGET, &mut reading).await.ok();
        measured.push(Measured {
            request,
            reading,
            early,
            elapsed: started.elapsed(),
        });
    }
    // A classifier regression may also wait on writer turns or SQL capacity.
    // Release every test-owned blocker before joining and asserting anything.
    drop(holder);
    drop(turns);
    release_capacity();
    let mut answers = Vec::new();
    for row in measured {
        let timely = row.early.is_some() && row.elapsed <= READ_BUDGET;
        let answer = match row.early {
            Some(answer) => answer.map_err(|error| error.to_string()),
            None => tokio::time::timeout(WATCHDOG, row.reading)
                .await
                .map_err(|error| error.to_string())
                .and_then(|answer| answer.map_err(|error| error.to_string())),
        };
        answers.push((row.request, answer, timely));
    }
    let late = answers
        .iter()
        .filter(|(_, _, timely)| !timely)
        .map(|(request, _, _)| request.label.as_str())
        .collect::<Vec<_>>();
    assert_eq!(turn_count, 2);
    for (request, answer, _) in &answers {
        assert_eq!(
            without_now(answer.as_ref().expect("socket worker failed").clone()),
            without_now(request.expected.clone()),
            "{} wire parity",
            request.label
        );
    }
    assert!(
        late.is_empty(),
        "MCP reads did not reply within100ms while AppState and both writer turns were held: {late:?}"
    );
}

/// Test setup can install independent handles before the immutable service is
/// captured. No request is allowed to discover those handles through AppState.
pub(super) fn recapture_plane(fixture: &mut Fixture) {
    fixture.plane = ControlPlane::new(fixture.state.lock().unwrap().frame_clock.clone());
}

pub(super) async fn pause(
    fixture: &mut Fixture,
    frame: Value,
) -> (OffLockGateHandle, tokio::task::JoinHandle<Value>) {
    let (gate, held) = OffLockGate::new();
    fixture.state.lock().unwrap().off_lock_gate = Some(gate);
    recapture_plane(fixture);
    let reading = tokio::spawn(socket_reply(
        fixture.state.clone(),
        fixture.plane.clone(),
        frame,
    ));
    let reached = tokio::time::timeout(WATCHDOG, async {
        while !held.has_pending_arrival() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await;
    fixture.state.lock().unwrap().off_lock_gate = None;
    if reached.is_err() {
        held.release();
        let _ = reading.await;
        panic!("read never reached independently admitted query work");
    }
    (held, reading)
}

pub(super) async fn finish(
    fixture: &Fixture,
    gate: OffLockGateHandle,
    mut reading: tokio::task::JoinHandle<Value>,
    expected: Value,
) {
    let turns = fixture.held_turns().await;
    let holder = HeldApp::new(fixture.state.clone());
    gate.release();
    let early = tokio::time::timeout(READ_BUDGET, &mut reading).await;
    drop(holder);
    drop(turns);
    let timely = early.is_ok();
    let answer = match early {
        Ok(answer) => answer.unwrap(),
        Err(_) => tokio::time::timeout(WATCHDOG, reading)
            .await
            .expect("final validation remained blocked after release")
            .unwrap(),
    };
    assert_eq!(answer, expected);
    assert!(timely, "MCP final validation waited for the app mutex");
}
