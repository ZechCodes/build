use super::*;

/// Run one frame against shared state without blocking the calling test.
pub(in crate::app::tests) fn frame_on_a_thread(
    state: &Arc<Mutex<AppState>>,
    session_id: &'static str,
    method: &'static str,
    params: Value,
) -> std::sync::mpsc::Receiver<Value> {
    let (answered, answers) = std::sync::mpsc::channel();
    let state = Arc::clone(state);
    std::thread::spawn(move || {
        let peers = state.lock().unwrap().peers_slot();
        let response = dispatch_frame(
            &state,
            &peers,
            SessionSender::detached(session_id),
            req(method, params),
            FrameClock::new().frame(method),
        );
        let _ = answered.send(response);
    });
    answers
}

/// The pending rows one project's board is showing right now.
pub(in crate::app::tests) fn pending_on_the_board(board: &Value) -> Vec<Value> {
    board["result"]["pending"]
        .as_array()
        .unwrap_or_else(|| panic!("the board ships its pending rows: {board:?}"))
        .clone()
}

/// Two dispatches of the same words name the same branch, and the branch is
/// what they collide on: the ref is settled before either one runs git, so
/// the second is refused rather than racing the first into `git worktree
/// add` with the same slug.
#[test]
fn two_dispatches_of_one_instruction_cut_one_branch() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let project_id = app.project_at(0).id.clone();
    let (gate, gate_handle) = OffLockGate::new();
    app.off_lock_gate = Some(gate);
    let state = app.shared();

    let first = frame_on_a_thread(
        &state,
        "s-first",
        "branch.dispatch",
        json!({ "project_id": project_id, "instruction": "Add a health endpoint" }),
    );
    gate_handle.wait_for_arrival();

    let second = frame_on_a_thread(
        &state,
        "s-second",
        "branch.dispatch",
        json!({ "project_id": project_id, "instruction": "Add a health endpoint" }),
    )
    .recv_timeout(Duration::from_secs(10))
    .expect("the second dispatch is answered rather than queued behind the first");
    assert_eq!(second["ok"], false, "{second:?}");

    gate_handle.release();
    let first = first
        .recv_timeout(Duration::from_secs(30))
        .expect("the first dispatch answers");
    assert_eq!(first["ok"], true, "{first:?}");
    assert_eq!(
        first["result"]["branch"], "build/add-a-health-endpoint",
        "{first:?}"
    );
    let checkouts = state
        .lock()
        .unwrap()
        .scan_external_worktrees_now(&project_id)
        .unwrap();
    assert!(
        checkouts.is_empty(),
        "one dispatch, one checkout — and the run owns it: {checkouts:?}"
    );
    assert_eq!(
        state.lock().unwrap().runs.len(),
        1,
        "the refused dispatch opened a run of its own"
    );
}

/// A dispatch cuts a branch, checks out the whole repository into it and
/// writes a checkpoint commit — all of it git, and none of it holding the
/// daemon still.
#[test]
fn branch_dispatch_cuts_its_branch_with_the_state_lock_free() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let project_id = app.project_at(0).id.clone();
    let run_id = adopted_run(&mut app, &repo, dir.path(), "already-here");
    let (gate, gate_handle) = OffLockGate::new();
    app.off_lock_gate = Some(gate);
    let state = app.shared();

    let dispatched = frame_on_a_thread(
        &state,
        "s-dispatch",
        "branch.dispatch",
        json!({ "project_id": project_id, "instruction": "Add a health endpoint" }),
    );
    gate_handle.wait_for_arrival();
    assert!(
        state.try_lock().is_ok(),
        "the dispatch is holding the app mutex through its git"
    );

    let board = frame_on_a_thread(&state, "s-board", "board.list", json!({}))
        .recv_timeout(Duration::from_secs(10))
        .expect("the board answers while the dispatch cuts its branch");
    let pending = pending_on_the_board(&board);
    assert_eq!(pending.len(), 1, "{board:?}");
    assert_eq!(pending[0]["state"], "creating", "{pending:?}");
    let posted = frame_on_a_thread(
        &state,
        "s-post",
        "thread.post",
        json!({ "entity_id": run_id, "body": "carry on" }),
    )
    .recv_timeout(Duration::from_secs(10))
    .expect("a message is answered while the dispatch cuts its branch");
    assert_eq!(posted["ok"], true, "{posted:?}");

    gate_handle.release();
    let dispatched = dispatched
        .recv_timeout(Duration::from_secs(30))
        .expect("the dispatch answers once its git is done");
    assert_eq!(dispatched["ok"], true, "{dispatched:?}");
    assert_eq!(
        dispatched["result"]["branch"], "build/add-a-health-endpoint",
        "{dispatched:?}"
    );
    let board = frame_on_a_thread(&state, "s-after", "board.list", json!({}))
        .recv_timeout(Duration::from_secs(10))
        .expect("the board answers");
    assert!(
        pending_on_the_board(&board).is_empty(),
        "the placeholder outlived the run it stood for: {board:?}"
    );
}

/// A git read is the other thing that costs seconds on a big checkout —
/// `git status` walks the whole tree — and the review surfaces poll it. It
/// runs off the lock for the same reason a finish does.
#[test]
fn a_git_read_runs_with_the_state_lock_free() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let project_id = app.project_at(0).id.clone();
    let (gate, gate_handle) = OffLockGate::new();
    app.off_lock_gate = Some(gate);
    let state = app.shared();

    let status = frame_on_a_thread(
        &state,
        "s-status",
        "git.status",
        json!({ "project_id": project_id }),
    );
    gate_handle.wait_for_arrival();
    assert!(
        state.try_lock().is_ok(),
        "the git read is holding the app mutex"
    );

    gate_handle.release();
    let status = status
        .recv_timeout(Duration::from_secs(30))
        .expect("the read answers once its git work is done");
    assert_eq!(status["ok"], true, "{status:?}");
    assert_eq!(status["result"]["branch"], "main", "{status:?}");
}

/// Review is the product, and rendering a patch reads every changed blob.
/// It runs off the lock too.
#[test]
fn a_diff_render_runs_with_the_state_lock_free() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let project_id = app.project_at(0).id.clone();
    std::fs::write(repo.join("changed.txt"), "work\n").unwrap();
    let (gate, gate_handle) = OffLockGate::new();
    app.off_lock_gate = Some(gate);
    let state = app.shared();

    let diff = frame_on_a_thread(
        &state,
        "s-diff",
        "git.changeset_diff",
        json!({ "project_id": project_id, "paths": ["changed.txt"] }),
    );
    gate_handle.wait_for_arrival();
    assert!(
        state.try_lock().is_ok(),
        "the diff render is holding the app mutex"
    );

    gate_handle.release();
    let diff = diff
        .recv_timeout(Duration::from_secs(30))
        .expect("the diff answers once it is rendered");
    assert_eq!(diff["ok"], true, "{diff:?}");
    assert_eq!(diff["result"]["stat"]["files_changed"], 1, "{diff:?}");
}

/// Staleness: what the git work computed describes a checkout that is no
/// longer on the board, so its cache write is dropped rather than
/// resurrecting the entity it was about.
#[test]
fn a_git_mutation_whose_run_vanished_mid_work_drops_its_cache_write() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut app, &repo, dir.path(), "vanishing");
    let worktree = app.runs[&run_id].worktree.path.clone();
    std::fs::write(worktree.join("staged.txt"), "work\n").unwrap();
    let state = app.shared();

    let peers = state.lock().unwrap().peers_slot();
    let staged = dispatch_frame(
        &state,
        &peers,
        SessionSender::detached("s-stage"),
        req(
            "git.stage",
            json!({ "run_id": run_id, "paths": ["staged.txt"] }),
        ),
        FrameClock::new().frame("git.stage"),
    );
    assert_eq!(staged["ok"], true, "{staged:?}");

    // The gate goes on only now: the commit is the call to catch in flight.
    let (gate, gate_handle) = OffLockGate::new();
    state.lock().unwrap().off_lock_gate = Some(gate);
    let committed = frame_on_a_thread(
        &state,
        "s-commit",
        "git.commit",
        json!({ "run_id": run_id, "message": "work" }),
    );
    gate_handle.wait_for_arrival();
    // The run leaves the board while the commit is still running.
    {
        let mut app = state.lock().unwrap();
        app.runs.remove(&run_id).expect("the run was on the board");
        app.forget_run(&run_id);
    }

    gate_handle.release();
    let committed = committed
        .recv_timeout(Duration::from_secs(30))
        .expect("the commit answers");
    assert_eq!(committed["ok"], true, "{committed:?}");

    let app = state.lock().unwrap();
    assert!(
        app.board.attention().clock(&run_id).updated_at.is_none(),
        "the commit stamped a run that had already left the board"
    );
}
