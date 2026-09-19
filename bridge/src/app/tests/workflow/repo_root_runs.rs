use super::*;

// ---- the retired primary-checkout work path -------------------------------

/// Work happens in a workspace. The project's own checkout is what a
/// workspace is cut from, so adopting it is refused — and the refusal names
/// the way forward rather than the flag that is gone.
#[test]
fn run_adopt_refuses_the_projects_own_checkout() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();

    let refused = state.handle(req(
        "run.adopt",
        json!({ "project_id": project_id, "primary": true }),
    ));

    assert_eq!(refused["ok"], false, "{refused:?}");
    let error = refused["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("workspace"),
        "the refusal names workspaces as the way to work: {refused:?}"
    );
    assert!(state.runs.is_empty(), "nothing was minted: {refused:?}");
    assert!(
        repo.join("README.md").exists(),
        "and the repository was not touched"
    );
}

/// A run standing in the project's repository is what a store written before
/// workspaces holds. Abandon removes a run's checkout and delete prunes it —
/// and neither may remove the repository. The guard is the canonical path,
/// not a flag on the record, so it holds for a run this daemon never minted.
#[test]
fn discarding_a_run_standing_in_the_repository_never_removes_it() {
    for verb in ["run.abandon", "run.delete"] {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let run_id = adopted_run(&mut state, &repo, dir.path(), "was-a-worktree");
        // What a legacy repo-root adoption left behind: the run's checkout IS
        // the project's repository.
        let run = state.runs.get_mut(&run_id).unwrap();
        run.worktree.path = AppState::canonical_root(&repo);
        if verb == "run.delete" {
            // Only a terminal run can be deleted, and a native one is the arm
            // that would prune.
            run.run.state = RunState::Abandoned;
            run.adopted = false;
        }

        let discarded = state.handle(req(verb, json!({ "run_id": run_id })));

        assert_eq!(discarded["ok"], true, "{verb}: {discarded:?}");
        assert!(
            repo.join("README.md").exists(),
            "{verb} removed the project's repository"
        );
        assert!(repo.join(".git").exists(), "{verb} removed the repository");
    }
}

/// A merge lands the run's branch on the base branch through the project's
/// repository. For a run standing in that repository the target IS the
/// checkout being merged, so both merge actions are refused before any git
/// runs — and the repository is left exactly as it was found.
#[test]
fn run_git_action_refuses_to_merge_a_run_standing_in_the_repository() {
    for action in ["merge", "merge_push"] {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let run_id = adopted_run(&mut state, &repo, dir.path(), "was-a-worktree");
        let run = state.runs.get_mut(&run_id).unwrap();
        run.worktree.path = AppState::canonical_root(&repo);

        let refused = state.handle(req(
            "run.git_action",
            json!({ "run_id": run_id, "action": action }),
        ));

        assert_eq!(refused["ok"], false, "{action}: {refused:?}");
        let error = refused["error"].as_str().unwrap_or_default();
        assert!(
            error.contains("cannot be merged"),
            "{action} names what it refuses: {refused:?}"
        );
        assert!(
            repo.join("README.md").exists(),
            "{action} touched the project's repository"
        );
        assert_eq!(
            git_stdout(&repo, &["status", "--porcelain"]).unwrap_or_default(),
            "",
            "{action} left the repository's working tree dirty"
        );
    }
}

/// A run adopted on the repo root before workspaces is still a run: it is
/// restored, it answers, and the surfaces that would have removed its
/// checkout refuse to. Its retired `primary` flag has left the wire.
#[test]
fn a_persisted_repo_root_run_is_restored_without_its_retired_flag() {
    let (dir, repo) = init_repo();
    let root = AppState::canonical_root(&repo).display().to_string();
    {
        let store = crate::store::Store::new(dir.path().join("store")).expect("a fresh store");
        let mut record = legacy_repo_root_run(&root);
        record.id = "run-legacy-root".to_string();
        store.save_run(&record).expect("the legacy record is kept");
    }

    let mut state = qa_state(&repo, dir.path());

    let got = state.handle(req("run.get", json!({ "run_id": "run-legacy-root" })));
    assert_eq!(got["ok"], true, "the legacy run is restored: {got:?}");
    assert_eq!(got["result"]["worktree_path"], root, "{got:?}");
    assert!(
        got["result"].get("primary").is_none(),
        "the retired flag has left the wire: {got:?}"
    );
}

/// A store written before workspaces holds `row:<project_id>:primary` keys
/// for the row the project's own checkout had. That row is gone, so the key
/// names nothing: loading must ignore it rather than fail, and the first save
/// after it prunes the key away.
#[test]
fn a_stale_primary_row_key_is_ignored_and_pruned() {
    let (dir, repo) = init_repo();
    let stale = "row:proj-1:primary".to_string();
    {
        let store = crate::store::Store::new(dir.path().join("store")).expect("a fresh store");
        let mut attention = std::collections::HashMap::new();
        let mut cleared = crate::attention::Attention::default();
        cleared.dismiss_at_head(Some("deadbeef"));
        attention.insert(stale.clone(), cleared);
        let live = attention.keys().cloned().collect();
        store
            .save_attention(&attention, &live)
            .expect("the legacy key is kept");
    }

    let mut state = qa_state(&repo, dir.path());

    let board = state.handle(req("board.list", json!({})));
    assert_eq!(board["ok"], true, "the board still answers: {board:?}");
    state.persist_attention();
    let kept = crate::store::Store::new(dir.path().join("store"))
        .expect("the store reopens")
        .load_attention();
    assert!(
        !kept.contains_key(&stale),
        "the key names no row any client can reach, so it is pruned: {kept:?}"
    );
}

/// A run record shaped the way a repo-root adoption left one: its checkout is
/// the project's repository, and it carries no branch of its own.
fn legacy_repo_root_run(root: &str) -> crate::store::PersistedRun {
    crate::store::PersistedRun {
        id: "run-legacy-root".to_string(),
        plan_id: None,
        goal: "the repository".to_string(),
        project_path: root.to_string(),
        base_branch: "main".to_string(),
        state: RunState::Review,
        branch: "main".to_string(),
        worktree_name: "main".to_string(),
        worktree_path: root.to_string(),
        base_sha: None,
        stages: Vec::new(),
        current_stage_id: None,
        revising_stage_id: None,
        auto_advance: false,
        adopted: true,
        publication_attempt: None,
        provider: Default::default(),
        model: None,
        effort: None,
        agents: Vec::new(),
        legacy_thread: Default::default(),
        last_summary: None,
        last_error: None,
        created_at: crate::store::now_rfc3339(),
        updated_at: crate::store::now_rfc3339(),
        state_changed_at: None,
    }
}
