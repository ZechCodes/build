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
        let response = dispatch_frame(
            &state,
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

/// Creating a worktree can be expensive, so other frames must keep moving
/// while Git adds it.
#[test]
fn worktree_create_runs_git_worktree_add_with_the_state_lock_free() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let project_id = app.project_at(0).id.clone();
    let run_id = adopted_run(&mut app, &repo, dir.path(), "already-here");
    let (gate, gate_handle) = OffLockGate::new();
    app.off_lock_gate = Some(gate);
    let state = app.shared();

    let created = frame_on_a_thread(
        &state,
        "s-create",
        "worktree.create",
        json!({ "project_id": project_id, "name": "scratch" }),
    );
    gate_handle.wait_for_arrival();
    assert!(
        state.try_lock().is_ok(),
        "the create is holding the app mutex through its git"
    );

    let board = frame_on_a_thread(&state, "s-board", "board.list", json!({}))
        .recv_timeout(Duration::from_secs(10))
        .expect("the board answers while a checkout is being cut");
    assert_eq!(board["ok"], true, "{board:?}");
    let posted = frame_on_a_thread(
        &state,
        "s-post",
        "thread.post",
        json!({ "entity_id": run_id, "body": "carry on" }),
    )
    .recv_timeout(Duration::from_secs(10))
    .expect("a message is answered while a checkout is being cut");
    assert_eq!(posted["ok"], true, "{posted:?}");

    gate_handle.release();
    let created = created
        .recv_timeout(Duration::from_secs(30))
        .expect("the create answers once its git is done");
    assert_eq!(created["ok"], true, "{created:?}");
    assert!(
        std::path::Path::new(created["result"]["path"].as_str().unwrap()).is_dir(),
        "{created:?}"
    );
}

/// The board shows the checkout from the moment it is asked for, under the
/// id it will settle as — not once the git returns.
#[test]
fn a_creating_worktree_is_on_the_board_before_its_git_returns() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let project_id = app.project_at(0).id.clone();
    let (gate, gate_handle) = OffLockGate::new();
    app.off_lock_gate = Some(gate);
    let state = app.shared();

    let created = frame_on_a_thread(
        &state,
        "s-create",
        "worktree.create",
        json!({ "project_id": project_id, "name": "Scratch Space" }),
    );
    gate_handle.wait_for_arrival();

    let board = frame_on_a_thread(&state, "s-board", "board.list", json!({}))
        .recv_timeout(Duration::from_secs(10))
        .expect("the board answers while the checkout is being cut");
    let pending = pending_on_the_board(&board);
    assert_eq!(pending.len(), 1, "{board:?}");
    assert_eq!(pending[0]["state"], "creating", "{pending:?}");
    assert_eq!(pending[0]["title"], "Scratch Space", "{pending:?}");
    assert_eq!(pending[0]["project_id"], json!(project_id), "{pending:?}");
    assert_eq!(
        pending[0]["project"],
        json!(state.lock().unwrap().project_at(0).name),
        "a row the board renders says which project it belongs to: {pending:?}"
    );
    let placeholder = pending[0]["entity_id"].as_str().unwrap().to_string();

    gate_handle.release();
    let created = created
        .recv_timeout(Duration::from_secs(30))
        .expect("the create answers once its git is done");
    assert_eq!(created["ok"], true, "{created:?}");
    assert_eq!(
        created["result"]["worktree_id"],
        json!(placeholder),
        "the checkout settled under the id its row was standing in for: {created:?}"
    );
    let board = frame_on_a_thread(&state, "s-after", "board.list", json!({}))
        .recv_timeout(Duration::from_secs(10))
        .expect("the board answers");
    assert!(
        pending_on_the_board(&board).is_empty(),
        "the placeholder outlived the record it stood for: {board:?}"
    );
}

/// A create whose git failed leaves nothing at all: no row on the board, no
/// claim on the name, and the error the git gave.
#[test]
fn a_create_that_fails_rolls_its_reservation_back_and_leaves_no_row() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    // A base branch this repository does not have: `git worktree add` has
    // nothing to cut from.
    state.project_at_mut(0).base_branch = "no-such-base".to_string();

    let failed = state.handle(req(
        "worktree.create",
        json!({ "project_id": project_id, "name": "doomed" }),
    ));

    assert_eq!(failed["ok"], false, "{failed:?}");
    assert!(
        state.pending_rows.is_empty(),
        "the failed create left its row on the board"
    );
    let board = state.handle(req("board.list", json!({})));
    assert!(pending_on_the_board(&board).is_empty(), "{board:?}");
    // And the name is free: the retry is not refused by the row of the
    // attempt that failed.
    state.project_at_mut(0).base_branch = "main".to_string();
    let retried = state.handle(req(
        "worktree.create",
        json!({ "project_id": project_id, "name": "doomed" }),
    ));
    assert_eq!(retried["ok"], true, "{retried:?}");
}

/// The decide phase guesses the checkout's id from the path its slug will
/// take, and `WorktreeManager` suffixes a slug something is already using.
/// The answer carries both ids so a client showing the placeholder replaces
/// that row rather than adding a second one beside it.
#[test]
fn a_suffixed_slug_settles_the_placeholder_under_its_real_id() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();

    let first = state.handle(req(
        "worktree.create",
        json!({ "project_id": project_id, "name": "scratch" }),
    ));
    assert_eq!(first["ok"], true, "{first:?}");
    assert_eq!(
        first["result"]["pending_worktree_id"], first["result"]["worktree_id"],
        "an unobstructed slug settles under the id its row carried: {first:?}"
    );

    let second = state.handle(req(
        "worktree.create",
        json!({ "project_id": project_id, "name": "scratch" }),
    ));
    assert_eq!(second["ok"], true, "{second:?}");
    assert_eq!(second["result"]["branch"], "build/scratch-2", "{second:?}");
    assert_ne!(
        second["result"]["pending_worktree_id"], second["result"]["worktree_id"],
        "the suffixed checkout settled under the placeholder's id: {second:?}"
    );
    assert_eq!(
        second["result"]["pending_worktree_id"], first["result"]["worktree_id"],
        "the placeholder stood at the path the first create took: {second:?}"
    );
    assert!(
        state.pending_rows.is_empty(),
        "a settled create left its row behind"
    );
}

/// A second create of a name already being cut is refused rather than
/// racing the first one's `git worktree add`.
#[test]
fn a_second_create_of_a_name_being_cut_is_refused() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let project_id = app.project_at(0).id.clone();
    let (gate, gate_handle) = OffLockGate::new();
    app.off_lock_gate = Some(gate);
    let state = app.shared();

    let created = frame_on_a_thread(
        &state,
        "s-create",
        "worktree.create",
        json!({ "project_id": project_id, "name": "scratch" }),
    );
    gate_handle.wait_for_arrival();

    let second = frame_on_a_thread(
        &state,
        "s-second",
        "worktree.create",
        json!({ "project_id": project_id, "name": "scratch" }),
    )
    .recv_timeout(Duration::from_secs(10))
    .expect("the second create is answered rather than queued behind the first");
    assert_eq!(second["ok"], false, "{second:?}");

    gate_handle.release();
    let created = created
        .recv_timeout(Duration::from_secs(30))
        .expect("the first create answers");
    assert_eq!(created["ok"], true, "{created:?}");
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

/// A create and a dispatch claim branches out of one namespace, so they
/// collide with each other too: the dispatch names `build/scratch`, which
/// is the branch the create in flight is cutting.
#[test]
fn a_dispatch_onto_a_branch_being_created_is_refused() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let project_id = app.project_at(0).id.clone();
    let (gate, gate_handle) = OffLockGate::new();
    app.off_lock_gate = Some(gate);
    let state = app.shared();

    let created = frame_on_a_thread(
        &state,
        "s-create",
        "worktree.create",
        json!({ "project_id": project_id, "name": "scratch" }),
    );
    gate_handle.wait_for_arrival();

    let dispatched = frame_on_a_thread(
        &state,
        "s-dispatch",
        "branch.dispatch",
        json!({
            "project_id": project_id,
            "branch": "build/scratch",
            "instruction": "pick this up",
        }),
    )
    .recv_timeout(Duration::from_secs(10))
    .expect("the dispatch is answered rather than queued behind the create");
    assert_eq!(dispatched["ok"], false, "{dispatched:?}");

    gate_handle.release();
    let created = created
        .recv_timeout(Duration::from_secs(30))
        .expect("the create answers");
    assert_eq!(created["ok"], true, "{created:?}");
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
        "project.diff",
        json!({ "project_id": project_id }),
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

    let staged = dispatch_frame(
        &state,
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
    let released = dispatch_frame(
        &state,
        SessionSender::detached("s-release"),
        req("run.release", json!({ "run_id": run_id })),
        FrameClock::new().frame("run.release"),
    );
    assert_eq!(released["ok"], true, "{released:?}");

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
