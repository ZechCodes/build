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
