use super::*;

/// A daemon behind the shared `Arc` with one bare checkout to finish, and
/// the gate that holds that finish inside its git phase.
fn daemon_with_a_checkout_to_finish(
    repo: &std::path::Path,
    dir: &std::path::Path,
    name: &str,
) -> (
    Arc<Mutex<AppState>>,
    String,
    String,
    PathBuf,
    OffLockGateHandle,
) {
    let mut app = qa_state(repo, dir);
    let project_id = app.projects[0].id.clone();
    let path = add_external_worktree(repo, dir, name, name);
    let worktree_id = external_id(&mut app, &project_id, Some(name));
    let (gate, handle) = OffLockGate::new();
    app.off_lock_gate = Some(gate);
    (app.shared(), project_id, worktree_id, path, handle)
}

/// One frame over the shared state, on its own thread — the caller keeps
/// the receiver so a frame that never comes back is an assertion, not a
/// hung test run.
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

/// The convoy this whole split exists to prevent: a finish's git work —
/// seconds of `git worktree remove` on a big checkout — must not hold the
/// app mutex, or every other frame queues behind one deletion.
#[test]
fn a_finish_runs_its_git_work_with_the_state_lock_free() {
    let (dir, repo) = init_repo();
    let (state, project_id, worktree_id, path, gate) =
        daemon_with_a_checkout_to_finish(&repo, dir.path(), "slow");

    let finished = frame_on_a_thread(
        &state,
        "s-finish",
        "worktree.finish",
        json!({ "project_id": project_id, "worktree_id": worktree_id, "action": "cleanup" }),
    );
    gate.wait_for_arrival();

    assert!(
        state.try_lock().is_ok(),
        "the finish is holding the app mutex through its git work"
    );
    let read = frame_on_a_thread(&state, "s-read", "project.list", json!({}));
    let answered = read
        .recv_timeout(Duration::from_secs(5))
        .expect("an unrelated read is answered while the finish runs");
    assert_eq!(answered["ok"], true, "{answered:?}");

    gate.release();
    let finished = finished
        .recv_timeout(Duration::from_secs(30))
        .expect("the finish answers once its git work is done");
    assert_eq!(finished["ok"], true, "{finished:?}");
    assert_eq!(finished["result"]["action"], "cleanup");
    assert!(!path.exists(), "the checkout was removed");
}

/// The pending rows one project's board is showing right now.
pub(in crate::app::tests) fn pending_on_the_board(board: &Value) -> Vec<Value> {
    board["result"]["pending"]
        .as_array()
        .unwrap_or_else(|| panic!("the board ships its pending rows: {board:?}"))
        .clone()
}

/// A `git worktree add` is a full checkout of the repository — minutes on a
/// large one — and every other frame has to keep moving while it runs.
#[test]
fn worktree_create_runs_git_worktree_add_with_the_state_lock_free() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let project_id = app.projects[0].id.clone();
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
    let project_id = app.projects[0].id.clone();
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
        json!(state.lock().unwrap().projects[0].name),
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
    let project_id = state.projects[0].id.clone();
    // A base branch this repository does not have: `git worktree add` has
    // nothing to cut from.
    state.projects[0].base_branch = "no-such-base".to_string();

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
    state.projects[0].base_branch = "main".to_string();
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
    let project_id = state.projects[0].id.clone();

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
    let project_id = app.projects[0].id.clone();
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
    let project_id = app.projects[0].id.clone();
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
    let project_id = app.projects[0].id.clone();
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
    let project_id = app.projects[0].id.clone();
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

/// An approved Issue with two approved stages, ready to be implemented.
pub(in crate::app::tests) fn approved_issue(app: &mut AppState, goal: &str) -> String {
    let issue = app.handle(req("issue.create", json!({ "goal": goal })));
    let issue_id = issue["result"]["issue_id"]
        .as_str()
        .expect("the issue was filed")
        .to_string();
    for stage_id in ["first-half", "second-half"] {
        app.handle(req(
            "issue.stage_approve",
            json!({ "issue_id": issue_id, "stage_id": stage_id }),
        ));
    }
    app.handle(req("issue.approve", json!({ "issue_id": issue_id })));
    issue_id
}

/// `run.create` cuts a checkout, scaffolds it and commits the Issue's plan
/// docs into it — `git worktree add` plus two commits, seconds of it on a
/// real repository. Every other frame goes through meanwhile.
#[test]
fn run_create_opens_its_implementation_with_the_state_lock_free() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let project_id = app.projects[0].id.clone();
    let issue_id = approved_issue(&mut app, "implement off the lock");
    let run_id = adopted_run(&mut app, &repo, dir.path(), "already-here");
    // A board that has been looked at once, so the assertion below is
    // about a list that exists.
    app.scan_external_worktrees_now(&project_id).unwrap();
    let (gate, gate_handle) = OffLockGate::new();
    app.off_lock_gate = Some(gate);
    let state = app.shared();

    let created = frame_on_a_thread(
        &state,
        "s-run",
        "run.create",
        json!({ "plan_id": issue_id }),
    );
    gate_handle.wait_for_arrival();
    assert!(
        state.try_lock().is_ok(),
        "run.create is holding the app mutex through its git"
    );

    let board = frame_on_a_thread(&state, "s-board", "board.list", json!({}))
        .recv_timeout(Duration::from_secs(10))
        .expect("the board answers while the implementation checkout is being cut");
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
    .expect("a message is answered while the implementation checkout is being cut");
    assert_eq!(posted["ok"], true, "{posted:?}");

    gate_handle.release();
    let created = created
        .recv_timeout(Duration::from_secs(30))
        .expect("run.create answers once its git is done");
    assert_eq!(created["ok"], true, "{created:?}");
    assert!(
        created["result"]["run_id"].as_str().is_some(),
        "{created:?}"
    );
    let board = frame_on_a_thread(&state, "s-after", "board.list", json!({}))
        .recv_timeout(Duration::from_secs(10))
        .expect("the board answers");
    assert!(
        pending_on_the_board(&board).is_empty(),
        "the placeholder outlived the run it stood for: {board:?}"
    );
    let app = state.lock().unwrap();
    let checkout = crate::worktree::canonical_root(
        &app.runs[created["result"]["run_id"].as_str().expect("a run opened")]
            .worktree
            .path,
    );
    assert!(
        !app.projects[0]
            .external_scan
            .as_ref()
            .is_some_and(|cache| cache.worktrees.iter().any(|w| w.path == checkout)),
        "the run's own checkout is on the board as an unbound card too"
    );
}

/// `run.create` into a checkout a run already owns makes two commits there
/// — the checkpoint that keeps whatever the branch was carrying its own
/// legible commit, and the Issue's docs on top as the review baseline.
/// Both against a checkout that may be huge, so both are off the lock.
#[test]
fn run_create_into_an_existing_checkout_checkpoints_it_with_the_state_lock_free() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let issue_id = approved_issue(&mut app, "implement where the work started");
    let target = adopted_run(&mut app, &repo, dir.path(), "already-started");
    let elsewhere = adopted_run(&mut app, &repo, dir.path(), "somewhere-else");
    let worktree_id = worktree_id_of_run(&app, &target);
    let checkout = app.runs[&target].worktree.path.clone();
    // What the branch was carrying before Build was handed it, so the
    // checkpoint commit has something to make.
    std::fs::write(checkout.join("half-done.txt"), "started by hand\n").unwrap();
    let (gate, gate_handle) = OffLockGate::new();
    app.off_lock_gate = Some(gate);
    let state = app.shared();

    let created = frame_on_a_thread(
        &state,
        "s-run",
        "run.create",
        json!({ "plan_id": issue_id, "worktree_id": worktree_id }),
    );
    gate_handle.wait_for_arrival();
    assert!(
        state.try_lock().is_ok(),
        "run.create is holding the app mutex through its checkpoint"
    );
    let posted = frame_on_a_thread(
        &state,
        "s-post",
        "thread.post",
        json!({ "entity_id": elsewhere, "body": "carry on" }),
    )
    .recv_timeout(Duration::from_secs(10))
    .expect("a message is answered while the checkout is being checkpointed");
    assert_eq!(posted["ok"], true, "{posted:?}");

    gate_handle.release();
    let created = created
        .recv_timeout(Duration::from_secs(30))
        .expect("run.create answers once its git is done");
    assert_eq!(created["ok"], true, "{created:?}");
    assert_eq!(created["result"]["run_id"], target, "{created:?}");

    let app = state.lock().unwrap();
    let base_sha = app.runs[&target]
        .base_sha
        .clone()
        .expect("the docs commit is the review baseline");
    let log = Command::new("git")
        .args(["-C", checkout.to_str().unwrap(), "log", "--format=%H %s"])
        .output()
        .unwrap();
    let log = String::from_utf8(log.stdout).unwrap();
    let commits: Vec<&str> = log.lines().collect();
    let baseline = commits
        .iter()
        .position(|line| line.starts_with(&base_sha))
        .expect("the baseline commit is in the checkout's history");
    assert!(
        commits[baseline].ends_with("plan: implement where the work started"),
        "the baseline is not the docs commit: {log}"
    );
    assert!(
        commits[baseline + 1].ends_with("Checkpoint: before Build implements an Issue here"),
        "the branch's own work was swept into the docs commit: {log}"
    );
}

/// The single-active-writer gate on an Issue is the reservation, not the
/// run map: the run a `run.create` is opening is not in that map until its
/// git has landed, and a second `run.create` naming a checkout would pass
/// `ImplementableIssue::judge` meanwhile. Both rows claim the Issue, so the
/// second is refused where a second create of one slug is.
#[test]
fn a_second_implementation_of_an_issue_is_refused_while_the_first_is_being_cut() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let issue_id = approved_issue(&mut app, "one writer per issue");
    let target = adopted_run(&mut app, &repo, dir.path(), "already-started");
    let worktree_id = worktree_id_of_run(&app, &target);
    let (gate, gate_handle) = OffLockGate::new();
    app.off_lock_gate = Some(gate);
    let state = app.shared();

    let first = frame_on_a_thread(
        &state,
        "s-first",
        "run.create",
        json!({ "plan_id": issue_id }),
    );
    gate_handle.wait_for_arrival();

    let second = frame_on_a_thread(
        &state,
        "s-second",
        "run.create",
        json!({ "plan_id": issue_id, "worktree_id": worktree_id }),
    )
    .recv_timeout(Duration::from_secs(10))
    .expect("the second run.create is answered while the first cuts its checkout");
    assert_eq!(second["ok"], false, "{second:?}");
    assert!(
        second["error"]
            .as_str()
            .is_some_and(|error| error.contains("already creating")),
        "{second:?}"
    );

    gate_handle.release();
    let first = first
        .recv_timeout(Duration::from_secs(30))
        .expect("the first run.create answers once its git is done");
    assert_eq!(first["ok"], true, "{first:?}");
    let app = state.lock().unwrap();
    let implementing = app
        .runs
        .values()
        .filter(|run| {
            run.run.plan_id.as_ref().map(|p| p.0.as_str()) == Some(issue_id.as_str())
                && !run.run.state.is_terminal()
        })
        .count();
    assert_eq!(implementing, 1, "two runs are implementing one Issue");
    assert!(
        app.runs[&target].run.plan_id.is_none(),
        "the refused run.create bound the target checkout to the Issue anyway"
    );
}

/// The git ran and the epilogue did not, so the checkout it cut is on disk
/// under no run. It has to be on the board as the unbound card it is —
/// invisible until the next full rescan is how a minted checkout gets lost.
#[test]
fn an_implementation_whose_apply_fails_leaves_its_checkout_on_the_board() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let project_id = app.projects[0].id.clone();
    let issue_id = approved_issue(&mut app, "leave nothing hidden");
    // A board that has been looked at once, so the amendment has a list to
    // put the checkout back into.
    app.scan_external_worktrees_now(&project_id).unwrap();
    let (gate, gate_handle) = OffLockGate::new();
    app.off_lock_gate = Some(gate);
    let state = app.shared();

    let created = frame_on_a_thread(
        &state,
        "s-run",
        "run.create",
        json!({ "plan_id": issue_id }),
    );
    gate_handle.wait_for_arrival();
    // The Issue goes while the git runs: the apply has nothing left to
    // open a run around.
    state.lock().unwrap().plans.remove(&issue_id);
    gate_handle.release();
    let created = created
        .recv_timeout(Duration::from_secs(30))
        .expect("run.create answers once its git is done");
    assert_eq!(created["ok"], false, "{created:?}");

    let app = state.lock().unwrap();
    assert!(app.runs.is_empty(), "the failed apply opened a run");
    assert!(
        app.pending_rows.is_empty(),
        "the failed apply left its row on the board"
    );
    let checkout = crate::worktree::canonical_root(
        &dir.path()
            .join("wt")
            .join(&project_id)
            .join("leave-nothing-hidden"),
    );
    assert!(checkout.is_dir(), "the git that succeeded was undone");
    assert!(
        app.projects[0]
            .external_scan
            .as_ref()
            .is_some_and(|cache| cache.worktrees.iter().any(|w| w.path == checkout)),
        "the checkout it cut is invisible until the next full rescan: {:?}",
        app.projects[0]
            .external_scan
            .as_ref()
            .map(|cache| &cache.worktrees)
    );
}

/// The scheduler cuts the same checkout on the way to a stage, so it waits
/// off the lock too — and answers with the Issue, not the run, once it has.
#[test]
fn issue_implement_all_opens_its_implementation_with_the_state_lock_free() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let issue_id = approved_issue(&mut app, "schedule off the lock");
    let run_id = adopted_run(&mut app, &repo, dir.path(), "already-here");
    let (gate, gate_handle) = OffLockGate::new();
    app.off_lock_gate = Some(gate);
    let state = app.shared();

    let implemented = frame_on_a_thread(
        &state,
        "s-implement",
        "issue.implement_all",
        json!({ "issue_id": issue_id }),
    );
    gate_handle.wait_for_arrival();
    assert!(
        state.try_lock().is_ok(),
        "the scheduler is holding the app mutex through its git"
    );
    let posted = frame_on_a_thread(
        &state,
        "s-post",
        "thread.post",
        json!({ "entity_id": run_id, "body": "carry on" }),
    )
    .recv_timeout(Duration::from_secs(10))
    .expect("a message is answered while the implementation checkout is being cut");
    assert_eq!(posted["ok"], true, "{posted:?}");

    gate_handle.release();
    let implemented = implemented
        .recv_timeout(Duration::from_secs(30))
        .expect("the scheduler answers once its git is done");
    assert_eq!(implemented["ok"], true, "{implemented:?}");
    assert_eq!(
        implemented["result"]["issue_id"], issue_id,
        "a scheduled implementation answers with the Issue it advanced: {implemented:?}"
    );
    assert_eq!(
        implemented["result"]["implementation_lineage"]
            .as_array()
            .expect("the Issue reports its lineage")
            .len(),
        1,
        "{implemented:?}"
    );
}

/// The `done` socket's twin of [`frame_on_a_thread`]: the guard is taken
/// for the report, released for whatever git the report handed back, and
/// taken again to write the result down.
fn done_on_a_thread(
    state: &Arc<Mutex<AppState>>,
    entity_id: &str,
    report: DoneReport,
) -> std::sync::mpsc::Receiver<Result<Value, String>> {
    let (answered, answers) = std::sync::mpsc::channel();
    let state = Arc::clone(state);
    let entity_id = entity_id.to_string();
    std::thread::spawn(move || {
        let deferred = state.lock().unwrap().done_deferring(&entity_id, report);
        let settled = match deferred {
            Some(deferred) => {
                let done = deferred.run();
                state
                    .lock()
                    .unwrap()
                    .apply_deferred(MCP_CONTROL_METHOD, &Value::Null, done)
            }
            None => Ok(Value::Null),
        };
        let _ = answered.send(settled);
    });
    answers
}

/// A recovery agent's own report carries its Issue's scheduler on to the
/// stage it was recovering for, and that hop is a checkout. The socket
/// releases the guard for it, the way it already does for a router tool.
#[test]
fn a_recovery_report_advances_its_scheduler_with_the_state_lock_free() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let issue_id = approved_issue(&mut app, "recover, then carry on");
    let run = app.handle(req("run.create", json!({ "plan_id": issue_id })));
    let run_id = run_id_of(&run);
    let branch = app.runs[&run_id].worktree.branch();
    let worktree = app.runs[&run_id].worktree.path.clone();
    let head_sha = String::from_utf8(
        Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&worktree)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    assert!(Command::new("git")
        .args([
            "worktree",
            "remove",
            "--force",
            "--",
            worktree.to_str().unwrap()
        ])
        .current_dir(&repo)
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["branch", "-D", "--", &branch])
        .current_dir(&repo)
        .status()
        .unwrap()
        .success());
    app.handle(req(
        "issue.implement_stage",
        json!({ "issue_id": issue_id, "stage_id": "second-half" }),
    ));
    let recovery_id = app.runs[&run_id].recovery.as_ref().unwrap().id.clone();
    assert!(Command::new("git")
        .args(["branch", &branch, &head_sha])
        .current_dir(&repo)
        .status()
        .unwrap()
        .success());
    let other_run = adopted_run(&mut app, &repo, dir.path(), "already-here");
    let (gate, gate_handle) = OffLockGate::new();
    app.off_lock_gate = Some(gate);
    let state = app.shared();

    let settled = done_on_a_thread(
        &state,
        &run_id,
        DoneReport {
            phase: DonePhase::Recover,
            status: DoneStatus::Completed,
            summary: "exact branch recovered".into(),
            outputs: DoneOutputs {
                recovery: Some(crate::mcp::RecoveryReport {
                    recovery_id,
                    recovered: true,
                    branch,
                    head_sha,
                    findings: "local reflog proved the exact tip".into(),
                }),
                ..DoneOutputs::default()
            },
        },
    );
    gate_handle.wait_for_arrival();
    assert!(
        state.try_lock().is_ok(),
        "the recovery report is holding the app mutex through its scheduler's git"
    );
    let posted = frame_on_a_thread(
        &state,
        "s-post",
        "thread.post",
        json!({ "entity_id": other_run, "body": "carry on" }),
    )
    .recv_timeout(Duration::from_secs(10))
    .expect("a message is answered while the recovered Issue's git runs");
    assert_eq!(posted["ok"], true, "{posted:?}");

    gate_handle.release();
    let settled = settled
        .recv_timeout(Duration::from_secs(30))
        .expect("the report settles once its git is done");
    assert!(settled.is_ok(), "{settled:?}");
    let app = state.lock().unwrap();
    assert_eq!(
        app.runs[&run_id].recovery.as_ref().unwrap().state,
        crate::run::RecoveryState::Succeeded
    );
    assert!(app.runs[&run_id].worktree.path.exists());
}

/// Approving a stage an armed Implement All is parked on is what starts
/// the whole implementation: the approval frame cuts the checkout, so it
/// hands that git to the drain like every other frame does.
#[test]
fn a_stage_approval_that_implements_cuts_its_checkout_with_the_state_lock_free() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let issue = app.handle(req(
        "issue.create",
        json!({ "goal": "approve, then build" }),
    ));
    let issue_id = issue["result"]["issue_id"]
        .as_str()
        .expect("the issue was filed")
        .to_string();
    app.handle(req("issue.approve", json!({ "issue_id": issue_id })));
    // Armed while stage one is unapproved: the scheduler parks, and the
    // approval below is what wakes it.
    let armed = app.handle(req("issue.implement_all", json!({ "issue_id": issue_id })));
    assert_eq!(armed["ok"], true, "{armed:?}");
    assert!(
        app.current_issue_implementation_id(&issue_id).is_none(),
        "the scheduler cut a checkout before its first stage was approved"
    );
    let run_id = adopted_run(&mut app, &repo, dir.path(), "already-here");
    let (gate, gate_handle) = OffLockGate::new();
    app.off_lock_gate = Some(gate);
    let state = app.shared();

    let approved = frame_on_a_thread(
        &state,
        "s-approve",
        "issue.stage_approve",
        json!({ "issue_id": issue_id, "stage_id": "first-half" }),
    );
    gate_handle.wait_for_arrival();
    assert!(
        state.try_lock().is_ok(),
        "the stage approval is holding the app mutex through its git"
    );
    let posted = frame_on_a_thread(
        &state,
        "s-post",
        "thread.post",
        json!({ "entity_id": run_id, "body": "carry on" }),
    )
    .recv_timeout(Duration::from_secs(10))
    .expect("a message is answered while the approval's checkout is being cut");
    assert_eq!(posted["ok"], true, "{posted:?}");

    gate_handle.release();
    let approved = approved
        .recv_timeout(Duration::from_secs(30))
        .expect("the approval answers once its git is done");
    assert_eq!(approved["ok"], true, "{approved:?}");
    assert_eq!(
        approved["result"]["issue_id"], issue_id,
        "a stage approval answers with the Issue it advanced: {approved:?}"
    );
    assert_eq!(
        approved["result"]["implementation_lineage"]
            .as_array()
            .expect("the Issue reports its lineage")
            .len(),
        1,
        "{approved:?}"
    );
}

/// A stage whose checkout was deleted outside Build puts it back with
/// `git worktree add` — and, when the branch is only on a remote, a fetch.
#[test]
fn implement_stage_restores_a_missing_checkout_with_the_state_lock_free() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let issue_id = approved_issue(&mut app, "restore off the lock");
    let run = app.handle(req("run.create", json!({ "plan_id": issue_id })));
    let run_id = run_id_of(&run);
    let worktree = app.runs[&run_id].worktree.path.clone();
    assert!(Command::new("git")
        .args([
            "worktree",
            "remove",
            "--force",
            "--",
            worktree.to_str().unwrap()
        ])
        .current_dir(&repo)
        .status()
        .unwrap()
        .success());
    let (gate, gate_handle) = OffLockGate::new();
    app.off_lock_gate = Some(gate);
    let state = app.shared();

    let implemented = frame_on_a_thread(
        &state,
        "s-stage",
        "issue.implement_stage",
        json!({ "issue_id": issue_id, "stage_id": "second-half" }),
    );
    gate_handle.wait_for_arrival();
    assert!(
        state.try_lock().is_ok(),
        "the restore is holding the app mutex through its git"
    );
    let got = frame_on_a_thread(
        &state,
        "s-get",
        "issue.get",
        json!({ "issue_id": issue_id }),
    )
    .recv_timeout(Duration::from_secs(10))
    .expect("the Issue answers while its checkout is being put back");
    assert_eq!(got["ok"], true, "{got:?}");

    gate_handle.release();
    let implemented = implemented
        .recv_timeout(Duration::from_secs(30))
        .expect("the stage answers once its git is done");
    assert_eq!(implemented["ok"], true, "{implemented:?}");
    assert!(worktree.exists(), "the checkout is back: {implemented:?}");
}

/// Spawning an agent scaffolds its checkout directory, and `git worktree
/// add` refuses a path that reappeared under it — which a restore reads as
/// a lost branch and answers by handing a healthy run to the recovery
/// agent. So a turn for a checkout being put back waits for it.
#[test]
fn a_turn_queued_for_a_restoring_checkout_never_sends_its_run_to_recovery() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let issue_id = approved_issue(&mut app, "restore under a queued turn");
    let run = app.handle(req("run.create", json!({ "plan_id": issue_id })));
    let run_id = run_id_of(&run);
    let worktree = app.runs[&run_id].worktree.path.clone();
    assert!(
        !app.pending_agent_turns.is_empty(),
        "the implementation queued its agent's first turn"
    );
    assert!(Command::new("git")
        .args([
            "worktree",
            "remove",
            "--force",
            "--",
            worktree.to_str().unwrap()
        ])
        .current_dir(&repo)
        .status()
        .unwrap()
        .success());
    let (gate, gate_handle) = OffLockGate::new();
    app.off_lock_gate = Some(gate);
    let state = app.shared();

    let implemented = frame_on_a_thread(
        &state,
        "s-stage",
        "issue.implement_stage",
        json!({ "issue_id": issue_id, "stage_id": "second-half" }),
    );
    gate_handle.wait_for_arrival();
    // This frame drains the queue while the restore is held open. The turn
    // for the run being restored is not its to deliver.
    let got = frame_on_a_thread(
        &state,
        "s-get",
        "issue.get",
        json!({ "issue_id": issue_id }),
    )
    .recv_timeout(Duration::from_secs(10))
    .expect("the Issue answers while its checkout is being put back");
    assert_eq!(got["ok"], true, "{got:?}");
    assert!(
        !worktree.exists(),
        "an agent was spawned into the checkout being restored: {got:?}"
    );

    gate_handle.release();
    let implemented = implemented
        .recv_timeout(Duration::from_secs(30))
        .expect("the stage answers once its git is done");
    assert_eq!(implemented["ok"], true, "{implemented:?}");
    let app = state.lock().unwrap();
    assert!(worktree.exists(), "the checkout is back");
    assert!(
        app.runs[&run_id].recovery.is_none(),
        "a healthy run was handed to the recovery agent: {:?}",
        app.runs[&run_id].recovery
    );
}

/// `plan.create` writes the planning workspace its agent works in — a
/// scratch docs dir and the `.build/` config in the primary checkout.
#[test]
fn plan_create_prepares_its_workspace_with_the_state_lock_free() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut app, &repo, dir.path(), "already-here");
    let (gate, gate_handle) = OffLockGate::new();
    app.off_lock_gate = Some(gate);
    let state = app.shared();

    let filed = frame_on_a_thread(
        &state,
        "s-plan",
        "plan.create",
        json!({ "goal": "draft off the lock" }),
    );
    gate_handle.wait_for_arrival();
    assert!(
        state.try_lock().is_ok(),
        "plan.create is holding the app mutex through its workspace"
    );
    let posted = frame_on_a_thread(
        &state,
        "s-post",
        "thread.post",
        json!({ "entity_id": run_id, "body": "carry on" }),
    )
    .recv_timeout(Duration::from_secs(10))
    .expect("a message is answered while the planning workspace is being written");
    assert_eq!(posted["ok"], true, "{posted:?}");

    gate_handle.release();
    let filed = filed
        .recv_timeout(Duration::from_secs(30))
        .expect("plan.create answers once its workspace is written");
    assert_eq!(filed["ok"], true, "{filed:?}");
    // The QA plan agent answers as soon as it is spawned, so a dispatched
    // plan is already at its review gate: what matters here is that it was
    // dispatched at all, not filed inert.
    assert_eq!(filed["result"]["state"], "plan_review", "{filed:?}");
}

/// Every other door to an Issue's planning agent writes the same workspace,
/// so every other door writes it off the lock too. A stage revision is the
/// one that costs most — the docs are re-materialized into the scratch dir
/// when the agent is not already working in it.
#[test]
fn a_stage_revision_writes_its_workspace_with_the_state_lock_free() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut app, &repo, dir.path(), "already-here");
    let issue = app.handle(req("plan.create", json!({ "goal": "revise off the lock" })));
    let issue_id = plan_id_of(&issue);
    app.handle(req(
        "plan.comment_add",
        json!({ "plan_id": issue_id, "stage_id": "first-half", "body": "split further" }),
    ));
    let (gate, gate_handle) = OffLockGate::new();
    app.off_lock_gate = Some(gate);
    let state = app.shared();

    let revised = frame_on_a_thread(
        &state,
        "s-revise",
        "plan.stage_send_notes",
        json!({ "plan_id": issue_id, "stage_id": "first-half" }),
    );
    gate_handle.wait_for_arrival();
    assert!(
        state.try_lock().is_ok(),
        "plan.stage_send_notes is holding the app mutex through its workspace"
    );
    let posted = frame_on_a_thread(
        &state,
        "s-post",
        "thread.post",
        json!({ "entity_id": run_id, "body": "carry on" }),
    )
    .recv_timeout(Duration::from_secs(10))
    .expect("a message is answered while the planning workspace is being written");
    assert_eq!(posted["ok"], true, "{posted:?}");

    gate_handle.release();
    let revised = revised
        .recv_timeout(Duration::from_secs(30))
        .expect("the revision answers once its workspace is written");
    assert_eq!(revised["ok"], true, "{revised:?}");
    // The QA plan agent answers the revision as soon as it is spawned, so
    // the stage is back at its gate with the comment resolved.
    assert_eq!(revised["result"]["state"], "plan_review", "{revised:?}");
    let app = state.lock().unwrap();
    assert!(
        app.pending_rows.is_empty(),
        "the revision left its row on the board"
    );
}

/// The same for a batch of plan notes, whose own message is durable before
/// any disk is asked for.
#[test]
fn plan_notes_write_their_workspace_with_the_state_lock_free() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut app, &repo, dir.path(), "already-here");
    let issue = app.handle(req(
        "plan.create",
        json!({ "goal": "take notes off the lock" }),
    ));
    let issue_id = plan_id_of(&issue);
    let (gate, gate_handle) = OffLockGate::new();
    app.off_lock_gate = Some(gate);
    let state = app.shared();

    let noted = frame_on_a_thread(
        &state,
        "s-notes",
        "plan.send_notes",
        json!({ "plan_id": issue_id, "comments": "tighten step two" }),
    );
    gate_handle.wait_for_arrival();
    assert!(
        state.try_lock().is_ok(),
        "plan.send_notes is holding the app mutex through its workspace"
    );
    {
        let app = state.lock().unwrap();
        let thread = &app.plans[&issue_id].agents.sole().thread;
        assert!(
            thread.items.iter().any(|item| matches!(
                item,
                crate::thread::ThreadItem::Message(message)
                    if message.body.contains("tighten step two")
            )),
            "the notes are durable before the workspace they are revised in"
        );
    }
    let posted = frame_on_a_thread(
        &state,
        "s-post",
        "thread.post",
        json!({ "entity_id": run_id, "body": "carry on" }),
    )
    .recv_timeout(Duration::from_secs(10))
    .expect("a message is answered while the planning workspace is being written");
    assert_eq!(posted["ok"], true, "{posted:?}");

    gate_handle.release();
    let noted = noted
        .recv_timeout(Duration::from_secs(30))
        .expect("the notes answer once their workspace is written");
    assert_eq!(noted["ok"], true, "{noted:?}");
    assert_eq!(noted["result"]["state"], "plan_review", "{noted:?}");
}

/// And for the first message to an inert Issue, which is what starts the
/// session it never had.
#[test]
fn an_inert_issues_first_message_starts_its_session_off_the_lock() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut app, &repo, dir.path(), "already-here");
    let issue = app.handle(req(
        "plan.create",
        json!({ "goal": "file me inert", "dispatch": false }),
    ));
    let issue_id = plan_id_of(&issue);
    assert_eq!(issue["result"]["state"], "created", "{issue:?}");
    let (gate, gate_handle) = OffLockGate::new();
    app.off_lock_gate = Some(gate);
    let state = app.shared();

    let said = frame_on_a_thread(
        &state,
        "s-say",
        "thread.post",
        json!({ "entity_id": issue_id, "body": "and here is what I meant" }),
    );
    gate_handle.wait_for_arrival();
    assert!(
        state.try_lock().is_ok(),
        "thread.post is holding the app mutex through the workspace it starts"
    );
    let posted = frame_on_a_thread(
        &state,
        "s-post",
        "thread.post",
        json!({ "entity_id": run_id, "body": "carry on" }),
    )
    .recv_timeout(Duration::from_secs(10))
    .expect("a message is answered while the planning workspace is being written");
    assert_eq!(posted["ok"], true, "{posted:?}");

    gate_handle.release();
    let said = said
        .recv_timeout(Duration::from_secs(30))
        .expect("the message answers once the session it started is open");
    assert_eq!(said["ok"], true, "{said:?}");
    assert_eq!(said["result"]["state"], "plan_review", "{said:?}");
    assert!(
        said["result"]["posted_sequence"].as_u64().is_some(),
        "the composer is told where its message landed: {said:?}"
    );
}

/// A git read is the other thing that costs seconds on a big checkout —
/// `git status` walks the whole tree — and the review surfaces poll it. It
/// runs off the lock for the same reason a finish does.
#[test]
fn a_git_read_runs_with_the_state_lock_free() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let project_id = app.projects[0].id.clone();
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
    let project_id = app.projects[0].id.clone();
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
        !app.entity_updated_at.contains_key(&run_id),
        "the commit stamped a run that had already left the board"
    );
}

/// The claim the off-lock split needs: while one finish is off deleting a
/// checkout, a second one for the same checkout refuses instead of racing
/// its `git worktree remove` — and the claim is released, so a retry after
/// it completes is the ordinary idempotent replay.
#[test]
fn a_second_finish_of_a_checkout_already_finishing_refuses() {
    let (dir, repo) = init_repo();
    let (state, project_id, worktree_id, path, gate) =
        daemon_with_a_checkout_to_finish(&repo, dir.path(), "contended");
    let params =
        json!({ "project_id": project_id, "worktree_id": worktree_id, "action": "cleanup" });

    let finished = frame_on_a_thread(&state, "s-first", "worktree.finish", params.clone());
    gate.wait_for_arrival();

    let refused = dispatch_frame(
        &state,
        SessionSender::detached("s-second"),
        req("worktree.finish", params.clone()),
        FrameClock::new().frame("worktree.finish"),
    );
    assert_eq!(refused["ok"], false, "{refused:?}");
    assert!(
        refused["error"]
            .as_str()
            .expect("the refusal says why")
            .contains("already finishing"),
        "{refused:?}"
    );
    assert!(path.exists(), "the refused finish removed nothing");

    gate.release();
    let finished = finished
        .recv_timeout(Duration::from_secs(30))
        .expect("the first finish completes");
    assert_eq!(finished["ok"], true, "{finished:?}");

    // A permit left waiting, so a replay that somehow reached the git phase
    // would answer rather than hang the run.
    gate.release();
    let replayed = dispatch_frame(
        &state,
        SessionSender::detached("s-third"),
        req("worktree.finish", params),
        FrameClock::new().frame("worktree.finish"),
    );
    assert_eq!(
        replayed["ok"], true,
        "the claim was released, so a replay is idempotent: {replayed:?}"
    );
}
