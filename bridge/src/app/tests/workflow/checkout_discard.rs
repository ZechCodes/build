use super::*;

// ---- letting go of a checkout: abandon and delete --------------------------

/// Removing a checkout is `remove_dir_all` over a whole working tree, and a
/// run being abandoned must not stop every other frame while it runs.
#[test]
fn run_abandon_removes_its_checkout_with_the_state_lock_free() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut app, &repo, dir.path(), "going-away");
    let checkout = app.runs[&run_id].worktree.path.clone();
    let (gate, gate_handle) = OffLockGate::new();
    app.off_lock_gate = Some(gate);
    let state = app.shared();

    let abandoned = frame_on_a_thread(
        &state,
        "s-abandon",
        "run.abandon",
        json!({ "run_id": run_id }),
    );
    gate_handle.wait_for_arrival();
    assert!(
        state.try_lock().is_ok(),
        "the abandon is holding the app mutex through its git"
    );
    let board = frame_on_a_thread(&state, "s-board", "board.list", json!({}))
        .recv_timeout(Duration::from_secs(10))
        .expect("the board answers while a checkout is being removed");
    assert_eq!(board["ok"], true, "{board:?}");

    gate_handle.release();
    let abandoned = abandoned
        .recv_timeout(Duration::from_secs(30))
        .expect("the abandon answers once its git is done");
    assert_eq!(abandoned["ok"], true, "{abandoned:?}");
    assert_eq!(abandoned["result"]["state"], "abandoned", "{abandoned:?}");
    assert!(!checkout.exists(), "the checkout is removed");
}

/// Whether a stage's commits ever left this machine is a bounded fetch and
/// two graph walks per stage, and it has to be asked while the refs still
/// stand — so it is the first thing the removal's own phase does.
#[test]
fn run_abandon_waits_for_its_agents_to_die_before_removing_the_checkout() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut app, &repo, dir.path(), "wedged-agent");
    let checkout = app.runs[&run_id].worktree.path.clone();
    let root = AppState::canonical_root(&checkout);
    let (death, death_handle) = OffLockGate::new();
    let agent_id = crate::agent::derived_agent_id(&run_id);
    app.session_registry.test_insert_tab(
        derived_agent_key(&root, &run_id),
        gated_tab(
            &root,
            gated_agent_role(&agent_id),
            GatedHarness::new().refusing_to_die_until(death),
        ),
    );
    let state = app.shared();

    let abandoned = frame_on_a_thread(
        &state,
        "s-abandon",
        "run.abandon",
        json!({ "run_id": run_id }),
    );
    death_handle.wait_for_arrival();
    assert!(
        checkout.exists(),
        "the checkout is not removed out from under a process still writing into it"
    );
    // The reap gives up after five seconds. A second frame must complete
    // while the agent is held at the gate, before that fallback can run.
    let board = frame_on_a_thread(&state, "s-board", "board.list", json!({}))
        .recv_timeout(Duration::from_secs(2))
        .expect("the wait for the agent is holding the app mutex");
    assert_eq!(board["ok"], true, "{board:?}");

    death_handle.release();
    let abandoned = abandoned
        .recv_timeout(Duration::from_secs(30))
        .expect("the abandon answers once the agent is reaped");
    assert_eq!(abandoned["ok"], true, "{abandoned:?}");
    assert!(!checkout.exists(), "the checkout is removed after the reap");
}

/// The wait is bounded and the removal is best-effort, as it has always
/// been: an agent that will not die never strands a run on the board.
#[test]
fn run_abandon_removes_the_checkout_anyway_when_an_agent_will_not_die() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut app, &repo, dir.path(), "never-dies");
    let checkout = app.runs[&run_id].worktree.path.clone();
    let root = AppState::canonical_root(&checkout);
    let (death, death_handle) = OffLockGate::new();
    let agent_id = crate::agent::derived_agent_id(&run_id);
    app.session_registry.test_insert_tab(
        derived_agent_key(&root, &run_id),
        gated_tab(
            &root,
            gated_agent_role(&agent_id),
            GatedHarness::new().refusing_to_die_until(death),
        ),
    );
    let state = app.shared();

    let abandoned = frame_on_a_thread(
        &state,
        "s-abandon",
        "run.abandon",
        json!({ "run_id": run_id }),
    )
    .recv_timeout(crate::orchestrator::CHECKOUT_REAP_WAIT + Duration::from_secs(25))
    .expect("the abandon gives up on the reap and lands anyway");
    assert_eq!(abandoned["ok"], true, "{abandoned:?}");
    assert_eq!(abandoned["result"]["state"], "abandoned", "{abandoned:?}");
    assert!(!checkout.exists(), "the checkout is removed anyway");

    death_handle.release();
}

/// A delete that is refused must leave the card exactly as it found it.
/// `run.adopt` and `run.delete` both claim the same checkout, and a
/// terminal run is precisely the owner an adoption walks past — so an
/// adoption in its git phase refuses the delete, and the run it refused is
/// still there, record and all, to be deleted once the adoption lands.
#[test]
fn a_delete_refused_by_a_running_adopt_keeps_the_runs_record() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let project_id = app.project_at(0).id.clone();
    let run_id = adopted_run(&mut app, &repo, dir.path(), "contested");
    let checkout = app.runs[&run_id].worktree.path.clone();
    // Only a terminal run can be deleted, and a terminal run is the one
    // owner an adoption of its checkout walks past.
    app.runs.get_mut(&run_id).unwrap().run.state = RunState::Abandoned;
    let worktree_id = crate::worktree::external_worktree_id(&AppState::canonical_root(&checkout));
    let (gate, gate_handle) = OffLockGate::new();
    app.off_lock_gate = Some(gate);
    let state = app.shared();

    let adopting = frame_on_a_thread(
        &state,
        "s-adopt",
        "run.adopt",
        json!({ "project_id": project_id, "worktree_id": worktree_id }),
    );
    gate_handle.wait_for_arrival();
    let deleted = frame_on_a_thread(
        &state,
        "s-delete",
        "run.delete",
        json!({ "run_id": run_id }),
    )
    .recv_timeout(Duration::from_secs(10))
    .expect("the delete is answered while the adoption's git runs");
    assert_eq!(
        deleted["ok"], false,
        "the checkout is claimed, so the delete waits its turn: {deleted:?}"
    );

    gate_handle.release();
    let adopted = adopting
        .recv_timeout(Duration::from_secs(30))
        .expect("the adoption answers once its git is done");
    // And it finds nothing to take over: the card the delete failed to
    // clear still binds that checkout, which is what a delete is for.
    assert_eq!(adopted["ok"], false, "{adopted:?}");

    let mut state = state.lock().unwrap();
    let got = state.handle(req("run.get", json!({ "run_id": run_id })));
    assert_eq!(
        got["ok"], true,
        "the refused delete left the card standing: {got:?}"
    );
    let persisted = state
        .store
        .as_ref()
        .expect("the QA daemon keeps a store")
        .load_all_runs()
        .expect("the store answers");
    assert!(
        persisted.iter().any(|run| run.id == run_id),
        "nor did it destroy the durable record it refused to delete"
    );
}

/// Clearing the card of a run minted around a checkout the user already had
/// must never touch that directory — the run goes, the files stay — and the
/// placeholder the verb stood up goes with it either way.
#[test]
fn run_delete_clears_an_adopted_card_and_leaves_the_checkout_standing() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut app, &repo, dir.path(), "theirs-to-keep");
    let checkout = app.runs[&run_id].worktree.path.clone();
    let branch = app.runs[&run_id].worktree.branch();
    // Only a terminal run can be deleted.
    app.runs.get_mut(&run_id).unwrap().run.state = RunState::Failed;
    let state = app.shared();

    let deleted = frame_on_a_thread(
        &state,
        "s-delete",
        "run.delete",
        json!({ "run_id": run_id }),
    )
    .recv_timeout(Duration::from_secs(30))
    .expect("the delete answers");
    assert_eq!(deleted["ok"], true, "{deleted:?}");

    let mut state = state.lock().unwrap();
    assert!(!state.runs.contains_key(&run_id), "the card is cleared");
    assert!(state.pending_rows.is_empty(), "the placeholder is retired");
    assert!(
        checkout.exists(),
        "clearing a card must never delete the user's files"
    );
    assert!(
        local_branch_exists(&repo, &branch).unwrap(),
        "nor the branch they were working on"
    );
    let board = state.handle(req("board.list", json!({})));
    assert!(pending_on_the_board(&board).is_empty(), "{board:?}");
}

#[test]
fn deleting_a_nonworkspace_run_keeps_its_project_message_after_restart() {
    let (dir, repo) = init_repo();
    let anchor = crate::session_summary::message_millis("2026-09-01T00:00:00Z").unwrap();
    {
        let mut state = qa_state(&repo, dir.path());
        let run_id = adopted_run(&mut state, &repo, dir.path(), "delete-history");
        let agent_id = primary_agent_id(&state, &run_id);
        let mut active = state.runs.remove(&run_id).unwrap();
        active
            .agents
            .by_id_mut(&agent_id)
            .unwrap()
            .thread
            .post_user("project history", None, "2026-09-01T00:00:00Z");
        active.run.state = RunState::Failed;
        state.finish_run_mutation(run_id.clone(), active).unwrap();
        let deleted = state.handle(req("run.delete", json!({"run_id":run_id})));
        assert_eq!(deleted["ok"], true, "{deleted:?}");
    }
    let restarted = qa_state(&repo, dir.path());
    let project = &restarted.project_list()["projects"][0];
    assert_eq!(project["session_started_ms"], anchor);
    assert_eq!(project["last_activity_ms"], anchor);
}

/// The run is off the board for the length of the removal, and the record
/// is the one thing the board can rebuild it from — so a delete the store
/// refuses puts the run back where the decide phase took it from, answers
/// with the refusal, and is retried like any other failed write. Dropping
/// the run there would clear the card with the record still standing,
/// and a restart would bring it back.
#[test]
fn a_delete_the_store_refuses_puts_the_run_back_on_the_board() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut state, &repo, dir.path(), "kept-by-refusal");
    // Only a terminal run can be deleted.
    state.runs.get_mut(&run_id).unwrap().run.state = RunState::Failed;
    state
        .store
        .as_ref()
        .expect("the QA daemon keeps a store")
        .fail_next_write();

    let deleted = state.handle(req("run.delete", json!({ "run_id": run_id })));

    assert_eq!(deleted["ok"], false, "{deleted:?}");
    assert!(
        deleted["error"]
            .as_str()
            .unwrap()
            .contains("injected store failure"),
        "{deleted:?}"
    );
    assert!(
        state.runs.contains_key(&run_id),
        "the run the store would not delete is back on the board"
    );
    let got = state.handle(req("run.get", json!({ "run_id": run_id })));
    assert_eq!(
        got["ok"], true,
        "the refused delete left the card standing: {got:?}"
    );
    assert!(state.pending_rows.is_empty(), "the placeholder is retired");
    let persisted = state
        .store
        .as_ref()
        .unwrap()
        .load_all_runs()
        .expect("the store answers");
    assert!(
        persisted.iter().any(|run| run.id == run_id),
        "the durable record survives the refusal"
    );

    let retried = state.handle(req("run.delete", json!({ "run_id": run_id })));
    assert_eq!(
        retried["ok"], true,
        "the delete is retryable once the store answers: {retried:?}"
    );
    assert!(
        !state.runs.contains_key(&run_id),
        "the retry clears the card"
    );
}

/// A run recovered after its repository moved off disk has no project
/// mapping at all — and that stale card is exactly what a delete is for. It
/// clears, and the directory it has no orchestrator to prune with is left
/// exactly where it stands.
#[test]
fn a_run_whose_project_is_gone_is_still_deletable() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut app, &repo, dir.path(), "orphaned-card");
    let checkout = app.runs[&run_id].worktree.path.clone();
    let run = app.runs.get_mut(&run_id).unwrap();
    // Only a terminal run can be deleted, and a native one is the arm that
    // would prune: the missing project is the only thing standing between
    // this delete and a `git worktree remove`.
    run.run.state = RunState::Failed;
    run.adopted = false;
    app.projects.unbind_live_entity(&run_id);
    let state = app.shared();

    let deleted = frame_on_a_thread(
        &state,
        "s-delete",
        "run.delete",
        json!({ "run_id": run_id }),
    )
    .recv_timeout(Duration::from_secs(30))
    .expect("the delete answers");
    assert_eq!(
        deleted["ok"], true,
        "a card whose project is gone is unremovable: {deleted:?}"
    );

    let state = state.lock().unwrap();
    assert!(!state.runs.contains_key(&run_id), "the card is cleared");
    assert!(state.pending_rows.is_empty(), "the placeholder is retired");
    assert!(
        checkout.exists(),
        "a delete with no orchestrator to prune with must leave the directory alone"
    );
}

/// Both discard verbs take the run off the board, so both drop the stat the
/// board cached for it at the same moment — a number computed against a
/// checkout that is being let go of is not one to serve again.
#[test]
fn discarding_a_run_drops_the_stat_the_board_cached_for_it() {
    for terminal in [false, true] {
        let (dir, repo) = init_repo();
        let mut app = qa_state(&repo, dir.path());
        let run_id = adopted_run(&mut app, &repo, dir.path(), "counted");
        app.board
            .diff_mut()
            .seed_run_stat(run_id.clone(), json!({ "files": 3 }));
        let verb = match terminal {
            false => "run.abandon",
            true => {
                app.runs.get_mut(&run_id).unwrap().run.state = RunState::Failed;
                "run.delete"
            }
        };
        let state = app.shared();

        let discarded = frame_on_a_thread(&state, "s-discard", verb, json!({ "run_id": run_id }))
            .recv_timeout(Duration::from_secs(30))
            .expect("the discard answers");
        assert_eq!(discarded["ok"], true, "{verb}: {discarded:?}");
        assert!(
            !state.lock().unwrap().board.diff().has_run_stat(&run_id),
            "{verb} served the board a stat read off a checkout it let go of"
        );
    }
}

#[test]
fn adopted_review_run_is_finishable_from_the_rail() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut state, &repo, dir.path(), "finishable-run");

    let board = state.handle(req("board.list", json!({})));
    let run = board["result"]["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|run| run["run_id"] == run_id)
        .unwrap();
    assert_eq!(run["state"], "review", "{run:?}");
    assert_eq!(run["can_finish"], true, "{run:?}");
    assert_eq!(run["stat"]["branch"], "finishable-run", "{run:?}");
}
