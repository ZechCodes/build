use super::*;

// ---- the primary checkout as a super-worktree -----------------------------

/// Adopt the project's primary checkout — the same verb, the same run, the
/// repo root instead of a worktree beside it.
fn adopted_primary_run(state: &mut AppState) -> String {
    let project_id = state.project_at(0).id.clone();
    let adopted = state.handle(req(
        "run.adopt",
        json!({ "project_id": project_id, "primary": true }),
    ));
    assert_eq!(adopted["ok"], true, "{adopted:?}");
    run_id_of(&adopted)
}

/// The primary checkout adopts exactly like an external worktree, with one
/// difference the client cannot enforce: the repo root has a stable
/// identity, so a reload or a second browser must converge on ONE owner.
#[test]
fn run_adopt_primary_owns_the_repo_root_and_is_idempotent() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();

    let adopted = state.handle(req(
        "run.adopt",
        json!({ "project_id": project_id, "primary": true }),
    ));
    assert_eq!(adopted["ok"], true, "{adopted:?}");
    assert_eq!(adopted["result"]["state"], "review", "{adopted:?}");
    assert_eq!(adopted["result"]["adopted"], true, "{adopted:?}");
    assert_eq!(adopted["result"]["primary"], true, "{adopted:?}");
    // The primary sits on the base branch — the one state external
    // adoption refuses, and the normal state here.
    assert_eq!(adopted["result"]["branch"], "main", "{adopted:?}");
    let run_id = run_id_of(&adopted);
    assert_eq!(
        AppState::canonical_root(std::path::Path::new(
            adopted["result"]["worktree_path"].as_str().unwrap()
        )),
        AppState::canonical_root(&repo),
        "the primary run works in the repo root: {adopted:?}"
    );

    let again = state.handle(req(
        "run.adopt",
        json!({ "project_id": project_id, "primary": true }),
    ));
    assert_eq!(again["ok"], true, "{again:?}");
    assert_eq!(
        run_id_of(&again),
        run_id,
        "the primary checkout has one owner, whoever asks again"
    );
    assert_eq!(
        state.runs.len(),
        1,
        "a second adoption must not mint a second owner"
    );
}

#[test]
fn adopting_a_cleared_primary_keeps_it_cleared_until_a_message() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    let _ = branch_row(&mut state, "main");
    state.handle(req(
        "entity.dismiss",
        json!({ "project_id": project_id.clone(), "primary": true }),
    ));

    let adopted = state.handle(req(
        "run.adopt",
        json!({ "project_id": project_id, "primary": true }),
    ));
    let run_id = run_id_of(&adopted);
    let row = work_item_row_for(&mut state, &run_id);
    assert_eq!(row["dismissed"], true, "adoption is not a message: {row:?}");

    let added = state.handle(req("agent.add", json!({ "entity_id": run_id })));
    assert_eq!(added["ok"], true, "{added:?}");
    let row = work_item_row_for(&mut state, &run_id);
    assert_eq!(
        row["dismissed"], true,
        "adding an agent is not a message: {row:?}"
    );
    primary_thread_mut(&mut state.runs.get_mut(&run_id).unwrap().agents).post_user(
        "please continue",
        None,
        now_rfc3339(),
    );
    let row = work_item_row_for(&mut state, &run_id);
    assert_eq!(row["dismissed"], false, "{row:?}");
}

/// Finishing archives a worktree and removes it. The primary checkout is
/// the repository; there is nothing to file away and everything to lose.
#[test]
fn run_finish_refuses_the_primary_checkout() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = adopted_primary_run(&mut state);

    let finished = state.handle(req(
        "run.finish",
        json!({ "run_id": run_id, "action": "cleanup" }),
    ));
    assert_eq!(finished["ok"], false, "{finished:?}");
    assert!(
        finished["error"]
            .as_str()
            .unwrap()
            .contains("the primary checkout cannot be finished"),
        "the refusal names the reason: {finished:?}"
    );
    assert!(repo.join("README.md").exists(), "the checkout is untouched");
    assert!(state.runs.contains_key(&run_id), "the run survives");
}

/// `run.finish` refuses the primary checkout, so its view must not offer
/// it: a Done button that can only fail is not an offer.
#[test]
fn a_primary_run_never_offers_can_finish() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = adopted_primary_run(&mut state);

    let got = state.handle(req("run.get", json!({ "run_id": run_id })));
    assert_eq!(got["ok"], true, "{got:?}");
    assert_eq!(got["result"]["state"], "review", "{got:?}");
    assert_eq!(
        got["result"]["can_finish"], false,
        "finish is refused for the primary checkout, so it is never offered: {got:?}"
    );
}

/// Merging the primary checkout would merge the base branch into itself —
/// meaningless at best, and at worst a merge whose target is the very
/// checkout being merged.
#[test]
fn run_git_action_refuses_to_merge_the_primary_checkout() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = adopted_primary_run(&mut state);

    for action in ["merge", "merge_push"] {
        let refused = state.handle(req(
            "run.git_action",
            json!({ "run_id": run_id, "action": action }),
        ));
        assert_eq!(refused["ok"], false, "{action}: {refused:?}");
        assert!(
            refused["error"]
                .as_str()
                .unwrap()
                .contains("the primary checkout cannot be merged"),
            "{action}: the refusal names the reason: {refused:?}"
        );
    }
    assert!(repo.join("README.md").exists(), "the checkout is untouched");
}

/// Abandon ends the run and takes its agent with it — but a primary run's
/// checkout is the repository. `WorktreeManager::remove` starts with
/// `remove_dir_all`, so this path has to skip it entirely, and `run.delete`
/// after it must not reach for it either.
#[test]
fn abandoning_a_primary_run_never_touches_the_checkout() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = adopted_primary_run(&mut state);
    let root = AppState::canonical_root(&repo);
    let (tab, _rx) = Tab::spawn_agent(
        run_id.clone(),
        crate::agent::derived_agent_id(&run_id),
        test_agent_session_request(
            AgentProvider::default(),
            warm_tui_spec(),
            root.clone(),
            terminal_size(120, 40),
        ),
    )
    .unwrap();
    let agent_pid = agent_pid(&tab).expect("a live agent");
    state.tabs.insert(derived_agent_key(&root, &run_id), tab);

    let abandoned = state.handle(req("run.abandon", json!({ "run_id": run_id })));
    assert_eq!(abandoned["ok"], true, "{abandoned:?}");
    assert_eq!(abandoned["result"]["state"], "abandoned", "{abandoned:?}");
    assert!(
        repo.join("README.md").exists(),
        "abandoning a primary run must never delete the repository"
    );
    assert!(
        !state.tabs.contains_key(&derived_agent_key(&root, &run_id)),
        "the agent goes with the owner that hosted it"
    );
    assert!(process_reaped(agent_pid), "the agent is killed AND reaped");

    let deleted = state.handle(req("run.delete", json!({ "run_id": run_id })));
    assert_eq!(deleted["ok"], true, "{deleted:?}");
    assert!(
        repo.join("README.md").exists(),
        "clearing the card must never delete the repository"
    );
}

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
fn run_abandon_judges_its_stages_with_the_state_lock_free() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let (issue_id, run_id) = planned_run_in_review(&mut app, "abandon judged off the lock");
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
    let board = frame_on_a_thread(&state, "s-board", "board.list", json!({}))
        .recv_timeout(Duration::from_secs(10))
        .expect("the board answers while the stages are being judged");
    assert_eq!(board["ok"], true, "{board:?}");

    gate_handle.release();
    let abandoned = abandoned
        .recv_timeout(Duration::from_secs(30))
        .expect("the abandon answers once its git is done");
    assert_eq!(abandoned["ok"], true, "{abandoned:?}");
    let stages = state
        .lock()
        .unwrap()
        .handle(req("issue.stages", json!({ "issue_id": issue_id })));
    assert!(
        stages["result"]["stages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|stage| stage["execution"] == "incomplete"
                && stage["invalidation_reason"].as_str().is_some()),
        "the verdict git gave off the lock is written onto the stages: {stages:?}"
    );
}

/// A child still creating files in a directory fails the `remove_dir_all`
/// walking it, so the order is kill, reap, THEN remove — and the reap is
/// waited out where a wedged harness parks this job and nothing else.
#[test]
fn run_abandon_waits_for_its_agents_to_die_before_removing_the_checkout() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut app, &repo, dir.path(), "wedged-agent");
    let checkout = app.runs[&run_id].worktree.path.clone();
    let root = AppState::canonical_root(&checkout);
    let (death, death_handle) = OffLockGate::new();
    let agent_id = crate::agent::derived_agent_id(&run_id);
    app.tabs.insert(
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
    assert!(
        state.try_lock().is_ok(),
        "the wait for the agent is holding the app mutex"
    );

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
    app.tabs.insert(
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
        app.run_stat_cache.insert(
            run_id.clone(),
            (std::time::Instant::now(), json!({ "files": 3 })),
        );
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
            !state.lock().unwrap().run_stat_cache.contains_key(&run_id),
            "{verb} served the board a stat read off a checkout it let go of"
        );
    }
}

/// Which checkout a run owns is read back off the run record's own paths,
/// so a restart cannot lose it — and the one-owner rule still holds against
/// a run this daemon never minted.
#[test]
fn a_recovered_primary_run_still_owns_the_checkout() {
    let (dir, repo) = init_repo();
    let run_id = {
        let mut state = qa_state(&repo, dir.path());
        adopted_primary_run(&mut state)
    };

    let mut restarted = qa_state(&repo, dir.path());
    let recovered = restarted.handle(req("run.get", json!({ "run_id": run_id })));
    assert_eq!(recovered["ok"], true, "{recovered:?}");
    assert_eq!(recovered["result"]["primary"], true, "{recovered:?}");
    let project_id = restarted.project_at(0).id.clone();
    let readopted = restarted.handle(req(
        "run.adopt",
        json!({ "project_id": project_id, "primary": true }),
    ));
    assert_eq!(
        run_id_of(&readopted),
        run_id,
        "the owner from before the restart is the owner after it"
    );
}

/// The checkout is the source of truth for a run's branch. Adoption saw
/// whatever the primary checkout had checked out that day; when the user
/// later switches it, the run must answer with the branch checked out NOW
/// — one row named by the live branch, never a phantom row named by a
/// branch nobody is on.
#[test]
fn a_primary_runs_branch_follows_the_checkout() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    git_in(&repo, &["checkout", "-b", "feature-era"]);
    let run_id = adopted_primary_run(&mut state);
    git_in(&repo, &["checkout", "main"]);

    assert_eq!(state.runs[&run_id].worktree.branch(), "main");

    let board = state.handle(req("board.list", json!({})));
    let branch_rows: Vec<&Value> = board["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["kind"] == "branch")
        .collect();
    assert_eq!(
        branch_rows.len(),
        1,
        "one checkout, one row: {branch_rows:?}"
    );
    assert_eq!(branch_rows[0]["branch"], "main", "{branch_rows:?}");
    assert_eq!(branch_rows[0]["run_id"], json!(run_id), "{branch_rows:?}");
}

/// A primary run is an owner like any other: the surfaces that make an
/// owner useful reach it through the same verbs.
#[test]
fn thread_post_reaches_the_primary_runs_thread() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = adopted_primary_run(&mut state);

    let posted = state.handle(req(
        "thread.post",
        json!({ "entity_id": run_id, "body": "look at the flaky test" }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");
    assert!(
        primary_thread(&state.runs[&run_id].agents).has_unread(),
        "the message is waiting on the primary run's own thread"
    );
    let items = posted["result"]["thread"]["items"].as_array().unwrap();
    assert!(
        items
            .iter()
            .any(|item| item["data"]["body"] == "look at the flaky test"),
        "{posted:?}"
    );
}

/// The primary run must never read as one more worktree row: the rail's
/// worktree list stays free of it, its view says what it is, and the main
/// row names its owner so a reload routes straight to it.
#[test]
fn the_primary_run_is_flagged_and_never_a_worktree_row() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    let external_run_id = adopted_run(&mut state, &repo, dir.path(), "feature-y");
    let run_id = adopted_primary_run(&mut state);

    let board = state.handle(req("board.list", json!({})));
    let runs = board["result"]["runs"].as_array().unwrap();
    let primary = runs
        .iter()
        .find(|run| run["run_id"] == run_id)
        .unwrap_or_else(|| panic!("the primary run is on the board: {board:?}"));
    assert_eq!(primary["primary"], true, "{primary:?}");
    let external = runs
        .iter()
        .find(|run| run["run_id"] == external_run_id)
        .expect("the external run is on the board");
    assert_eq!(
        external["primary"], false,
        "a worktree run is not the primary: {external:?}"
    );

    let root = AppState::canonical_root(&repo).display().to_string();
    assert!(
        board["result"]["external_worktrees"]
            .as_array()
            .unwrap()
            .iter()
            .all(|worktree| worktree["path"] != root),
        "the repo root is never an external worktree row: {board:?}"
    );

    let main_row = board["result"]["primary_changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["project_id"] == project_id)
        .unwrap_or_else(|| panic!("the main row is on the board: {board:?}"))
        .clone();
    assert_eq!(
        main_row["run_id"], run_id,
        "the main row names its owner: {main_row:?}"
    );
}

/// The Agent tab on the main surface: `agent.start` needs an owner, and
/// adoption is what gives the primary checkout one. The agent runs in the
/// repo root, keyed there like every other worktree's agent.
#[tokio::test]
async fn agent_start_opens_the_primary_checkouts_agent() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_qa_state_and_handler(&repo, dir.path());
    let project_id = state.lock().unwrap().project_at(0).id.clone();
    let adopted = call(
        &handler,
        "run.adopt",
        json!({ "project_id": project_id, "primary": true }),
    );
    assert_eq!(adopted["ok"], true, "{adopted:?}");
    let run_id = run_id_of(&adopted);

    // Adoption minted no agent, so the start is what creates one — on the
    // account's default harness, since nobody named another.
    let started = call(&handler, "agent.start", json!({ "id": run_id }));
    assert_eq!(started["ok"], true, "{started:?}");
    assert_eq!(
        started["result"]["term_id"],
        agent_tab_id(started["result"]["agent_id"].as_str().unwrap()),
        "the reply reserves the tab id the agent's own identity mints: {started:?}"
    );
    wait_for_deliveries(&state).await;
    let root = AppState::canonical_root(&repo);
    {
        let s = state.lock().unwrap();
        let roster = s.entity_agents(&run_id).expect("the adopted run");
        assert_eq!(roster.len(), 1, "the start created exactly one agent");
        assert_eq!(
            roster.primary().unwrap().choice.provider,
            AgentProvider::ClaudeAdk,
            "on the account's default harness"
        );
        assert!(
            s.tabs.contains_key(&primary_agent_key(&s, &root, &run_id)),
            "the primary checkout's agent is keyed on the repo root"
        );
    }
    let again = call(&handler, "agent.start", json!({ "id": run_id }));
    assert_eq!(again["ok"], true, "{again:?}");
    assert_eq!(
        again["result"]["term_id"], started["result"]["term_id"],
        "a second start addresses the agent the first one opened: {again:?}"
    );
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

#[test]
fn run_finish_cleans_up_and_archives_the_bound_worktree() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut state, &repo, dir.path(), "archive-finished-run");
    let project_id = state.project_at(0).id.clone();
    let worktree = state.runs[&run_id].worktree.path.clone();
    git_in(&worktree, &["add", "-A"]);
    git_in(&worktree, &["commit", "-m", "Finish adopted work"]);

    let finished = state.handle(req(
        "run.finish",
        json!({ "run_id": run_id, "action": "cleanup" }),
    ));
    assert_eq!(finished["ok"], true, "{finished:?}");
    assert!(!state.runs.contains_key(&run_id));
    assert!(!worktree.exists(), "Done removes the finished checkout");
    assert!(
        repo.join(".git/refs/heads/archive-finished-run").exists(),
        "cleanup preserves the branch"
    );

    let archive = state.handle(req("archive.list", json!({ "project_id": project_id })));
    let archived = archive["result"]["worktrees"].as_array().unwrap();
    assert_eq!(archived.len(), 1, "{archive:?}");
    assert_eq!(archived[0]["action"], "cleanup", "{archive:?}");
}

/// Done decides from what its own preflight found, not from the numbers the
/// last poll left on the board. The blocking claim that used to make a
/// finish wait for a fresh diffstat under the frame's worker is gone; the
/// rescan inside the finish job is what it always really decided on.
#[test]
fn run_finish_refuses_uncommitted_work_found_by_its_own_preflight() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut state, &repo, dir.path(), "preflight-run");
    let worktree = state.runs[&run_id].worktree.path.clone();
    git_in(&worktree, &["add", "-A"]);
    git_in(&worktree, &["commit", "-m", "Finish adopted work"]);

    // The board reads the checkout while it is clean, and caches that.
    let board = state.handle(req("board.list", json!({})));
    assert_eq!(board["ok"], true, "{board:?}");

    // Then work lands in it that no poll has seen.
    std::fs::write(worktree.join("unsaved.txt"), "not committed\n").unwrap();

    let finished = state.handle(req(
        "run.finish",
        json!({ "run_id": run_id, "action": "cleanup" }),
    ));
    assert_eq!(finished["ok"], false, "{finished:?}");
    assert!(
        finished["error"]
            .as_str()
            .unwrap_or_default()
            .contains("uncommitted"),
        "{finished:?}"
    );
    assert!(
        worktree.join("unsaved.txt").exists(),
        "a refused finish leaves the work where it is"
    );
    assert!(
        state.runs.contains_key(&run_id),
        "a refused finish puts the run back on the board"
    );
}

#[test]
fn finishing_a_planned_run_keeps_archived_lineage_for_plan_done() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (plan_id, run_id) = planned_run_in_review(&mut state, "archive planned run");
    let worktree = state.runs[&run_id].worktree.path.clone();
    git_in(&worktree, &["add", "-A"]);
    let staged = git_stdout(&worktree, &["diff", "--cached", "--name-only"]).unwrap();
    if !staged.trim().is_empty() {
        git_in(&worktree, &["commit", "-m", "Finish planned work"]);
    }

    let finished = state.handle(req(
        "run.finish",
        json!({ "run_id": run_id, "action": "cleanup" }),
    ));
    assert_eq!(finished["ok"], true, "{finished:?}");
    assert_eq!(state.runs[&run_id].run.state, RunState::Archived);
    assert!(!worktree.exists());

    let plan = state.handle(req("plan.get", json!({ "plan_id": plan_id })));
    assert_eq!(plan["result"]["can_archive"], true, "{plan:?}");
}

#[test]
fn merged_run_with_no_checkout_still_gets_done_to_leave_the_rail() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, run_id) = planned_run_in_review(&mut state, "dismiss merged run");
    let merged = state.handle(req(
        "run.git_action",
        json!({ "run_id": run_id, "action": "merge" }),
    ));
    assert_eq!(merged["result"]["state"], "merged", "{merged:?}");
    assert!(!state.runs[&run_id].worktree.path.exists());

    let board = state.handle(req("board.list", json!({})));
    let run = board["result"]["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|run| run["run_id"] == run_id)
        .unwrap();
    assert_eq!(run["can_finish"], true, "{run:?}");

    let finished = state.handle(req(
        "run.finish",
        json!({ "run_id": run_id, "action": "cleanup" }),
    ));
    assert_eq!(finished["ok"], true, "{finished:?}");
    assert_eq!(state.runs[&run_id].run.state, RunState::Archived);
    let board = state.handle(req("board.list", json!({})));
    assert!(board["result"]["runs"]
        .as_array()
        .unwrap()
        .iter()
        .all(|run| run["run_id"] != run_id));
}

#[test]
fn failed_run_finish_restores_the_live_run_for_retry() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut state, &repo, dir.path(), "retry-finish-run");
    let worktree = state.runs[&run_id].worktree.path.clone();

    // QA adoption writes .build/.gitignore, so cleanup must refuse this
    // dirty tree before any destructive step.
    let failed = state.handle(req(
        "run.finish",
        json!({ "run_id": run_id, "action": "cleanup" }),
    ));
    assert_eq!(failed["ok"], false, "{failed:?}");
    assert!(failed["error"]
        .as_str()
        .unwrap()
        .contains("requires no uncommitted"));
    assert!(state.runs.contains_key(&run_id), "the rail entry survives");
    assert!(worktree.exists(), "the checkout survives");
}

#[test]
fn run_delete_is_terminal_only() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, run_id) = planned_run_in_review(&mut state, "live");
    let live = state.handle(req("run.delete", json!({ "run_id": run_id })));
    assert!(
        live["error"].as_str().unwrap().contains("terminal runs"),
        "{live:?}"
    );
    state.handle(req("run.abandon", json!({ "run_id": run_id })));
    let gone = state.handle(req("run.delete", json!({ "run_id": run_id })));
    assert_eq!(gone["ok"], true, "{gone:?}");
}

#[test]
fn merge_cleanup_keep_keeps_the_worktree() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, run_id) = planned_run_in_review(&mut state, "keep it");
    let worktree = state.runs.get(&run_id).unwrap().worktree.path.clone();
    let merged = state.handle(req(
        "run.git_action",
        json!({ "run_id": run_id, "action": "merge", "cleanup": "keep" }),
    ));
    assert_eq!(merged["result"]["state"], "merged");
    assert!(worktree.exists(), "cleanup=keep keeps the worktree");
}
