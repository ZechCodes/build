use super::*;

// ---- git scope keyed on a run --------------------------------------------

#[test]
fn git_status_and_commit_scope_to_a_run() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, run_id) = planned_run_in_review(&mut state, "git me");
    // The run's build wrote result.txt (committed by QA merge path? no — it
    // is committed on review); git.status over the run scope succeeds.
    let status = state.handle(req("git.status", json!({ "run_id": run_id })));
    assert_eq!(status["ok"], true, "{status:?}");
    // Exactly-one-scope is enforced.
    let both = state.handle(req(
        "git.status",
        json!({ "run_id": run_id, "project_id": "proj-1" }),
    ));
    assert_eq!(both["ok"], false, "{both:?}");
}

#[test]
fn git_scope_selects_one_workspace_source_and_names_its_cache() {
    let (dir, repo) = init_repo();
    let (other_dir, other_repo) = init_repo();
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();
    state.workspaces.adopt_root(
        &project_id,
        "workspace-a".to_string(),
        "A".to_string(),
        repo.clone(),
        "source-1".to_string(),
        true,
    );
    state.workspaces.adopt_root(
        &project_id,
        "workspace-b".to_string(),
        "B".to_string(),
        other_repo.clone(),
        "source-1".to_string(),
        true,
    );
    std::fs::write(repo.join("same.txt"), "same contents\n").unwrap();
    std::fs::write(other_repo.join("same.txt"), "same contents\n").unwrap();

    let first_scope = json!({ "workspace_id": "workspace-a", "source_id": "source-1" });
    let second_scope = json!({ "workspace_id": "workspace-b", "source_id": "source-1" });
    let first = state.handle(req("git.status", first_scope.clone()));
    let second = state.handle(req("git.status", second_scope.clone()));
    assert_eq!(first["ok"], true, "{first:?}");
    assert_eq!(second["ok"], true, "{second:?}");
    assert_eq!(first["result"]["path"], repo.display().to_string());
    assert_eq!(second["result"]["path"], other_repo.display().to_string());
    assert_ne!(
        first["result"]["status_key"], second["result"]["status_key"],
        "two selected directories must never share a browser cache key"
    );
    let history = state.handle(req("git.log", first_scope.clone()));
    assert_eq!(history["ok"], true, "{history:?}");
    assert!(
        history["result"]["commits"]
            .as_array()
            .unwrap()
            .iter()
            .all(|commit| commit["unpushed"] == true && commit.get("ahead_of_base").is_none()),
        "workspace history uses publication semantics: {history:?}"
    );
    assert!(history["result"]["highlight_key"].is_string());
    let unchanged = state.handle(req(
        "git.status",
        json!({
            "workspace_id": "workspace-a",
            "source_id": "source-1",
            "if_status_key": first["result"]["status_key"],
        }),
    ));
    assert_eq!(unchanged["result"]["unchanged"], true, "{unchanged:?}");
    let first_diff = state.handle(req(
        "git.diff",
        json!({ "workspace_id": "workspace-a", "source_id": "source-1", "paths": ["same.txt"] }),
    ));
    let second_diff = state.handle(req(
        "git.diff",
        json!({ "workspace_id": "workspace-b", "source_id": "source-1", "paths": ["same.txt"] }),
    ));
    assert_ne!(
        first_diff["result"]["files"][0]["content_key"],
        second_diff["result"]["files"][0]["content_key"],
        "identical relative files in separate directories need separate patch cache keys"
    );

    drop(other_dir);
}

#[test]
fn workspace_git_scope_requires_an_exact_git_source() {
    let (dir, repo) = init_repo();
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();
    let source_path = dir.path().join("ordinary-source");
    std::fs::create_dir(&source_path).unwrap();
    let workspace = state
        .workspaces
        .begin(
            &project_id,
            "ordinary-workspace",
            &[crate::workspace::WorkspaceSource {
                id: "ordinary".to_string(),
                name: "Ordinary".to_string(),
                mount: "ordinary".to_string(),
                path: source_path,
                is_git: false,
                base_branch: "main".to_string(),
            }],
        )
        .unwrap();

    for params in [
        json!({ "workspace_id": workspace.id, "source_id": "ordinary" }),
        json!({ "workspace_id": "workspace-a" }),
        json!({ "source_id": "source-1" }),
        json!({
            "workspace_id": "workspace-a",
            "source_id": "source-1",
            "project_id": project_id,
        }),
    ] {
        let response = state.handle(req("git.status", params));
        assert_eq!(response["ok"], false, "{response:?}");
    }
}

// ---- git scope keyed on an external worktree ------------------------------

/// Cut an unbound worktree by hand and hand back (project_id, worktree_id,
/// path): a checkout Build did not make, which the scan names.
fn bare_worktree(
    state: &mut AppState,
    repo: &std::path::Path,
    dir: &std::path::Path,
    name: &str,
) -> (String, String, PathBuf) {
    let project_id = state.project_at(0).id.clone();
    let path = add_external_worktree(repo, dir, name, &format!("build/{name}"));
    let worktree_id = state
        .scan_external_worktrees_now(&project_id)
        .unwrap()
        .into_iter()
        .find(|worktree| worktree.path == crate::worktree::canonical_root(&path))
        .expect("the scan names the hand-made worktree")
        .id;
    (project_id, worktree_id, path)
}

/// The whole git GUI — status, staging, commit, history — works on a
/// worktree the same way it does on the primary checkout, and reads the
/// worktree's own tree rather than the project's.
#[test]
fn the_git_gui_scopes_to_an_external_worktree() {
    let (dir, repo) = init_repo();
    let mut state = git_gui_state(&dir, &repo);
    let (project_id, worktree_id, path) = bare_worktree(&mut state, &repo, dir.path(), "scratch");
    let scope = json!({ "project_id": project_id, "worktree_id": worktree_id });
    std::fs::write(path.join("only-here.txt"), "in the worktree\n").unwrap();

    let status = state.handle(req("git.status", scope.clone()));
    assert_eq!(status["ok"], true, "{status:?}");
    assert!(has_file_entry(&status["result"], "only-here.txt"));
    assert_eq!(status["result"]["branch"], json!("build/scratch"));

    // The project's own checkout is untouched by any of it.
    let primary = state.handle(req("git.status", json!({ "project_id": project_id })));
    assert!(!has_file_entry(&primary["result"], "only-here.txt"));

    let staged = state.handle(req(
        "git.stage",
        json!({ "project_id": project_id, "worktree_id": worktree_id, "paths": ["only-here.txt"] }),
    ));
    assert_eq!(staged["ok"], true, "{staged:?}");
    let committed = state.handle(req(
        "git.commit",
        json!({ "project_id": project_id, "worktree_id": worktree_id, "message": "in the worktree" }),
    ));
    assert_eq!(committed["ok"], true, "{committed:?}");

    // History is the worktree's, and the new commit is marked ahead of the
    // base branch — the same affordance a run's history carries.
    let log = state.handle(req("git.log", scope.clone()));
    let commits = log["result"]["commits"].as_array().unwrap();
    assert_eq!(commits[0]["subject"], json!("in the worktree"));
    assert_eq!(commits[0]["ahead_of_base"], json!(true));
    assert_eq!(commits[1]["ahead_of_base"], json!(false));

    // And the rail sees the commit without waiting out the scan cache.
    let listed = state.external_worktrees(&project_id).worktrees;
    let entry = listed.iter().find(|w| w.id == worktree_id).unwrap();
    assert_eq!(entry.unpushed, Some(1));
    assert_eq!(entry.uncommitted.files_changed, 0, "committed, so clean");
}

/// Branch operations name a checkout, and a worktree scope means THAT
/// worktree — never the project's primary checkout standing in for it.
#[test]
fn branch_operations_switch_the_worktree_they_are_scoped_to() {
    let (dir, repo) = init_repo();
    let mut state = git_gui_state(&dir, &repo);
    let (project_id, worktree_id, path) = bare_worktree(&mut state, &repo, dir.path(), "scratch");

    git_in(&path, &["branch", "side-quest"]);
    let checked_out = state.handle(req(
        "git.checkout_ref",
        json!({ "project_id": project_id, "worktree_id": worktree_id, "full_ref": "refs/heads/side-quest" }),
    ));
    assert_eq!(checked_out["ok"], true, "{checked_out:?}");
    assert_eq!(checked_out["result"]["branch"], json!("side-quest"));

    // The worktree moved; the project's checkout stayed on main.
    let head = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(&path)
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&head.stdout).trim(), "side-quest");
    let primary = state.handle(req("git.status", json!({ "project_id": project_id })));
    assert_eq!(primary["result"]["branch"], json!("main"));
}

/// With a checkout's registration pruned, nothing on disk says whether
/// teardown owns its branch, and the run either knows or does not. A run
/// Build dispatched works in a checkout Build cut, so it can vouch. An
/// adopted run's checkout may be one Build only borrowed, so it cannot,
/// and its recovery stops rather than restoring under a guess that would
/// hand somebody's branch to the next teardown.
#[test]
fn only_a_run_build_cut_its_own_checkout_for_vouches_for_its_branch() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut state, &repo, dir.path(), "borrowed");

    assert_eq!(
        unregistered_restore_for(state.runs.get(&run_id).unwrap()),
        crate::worktree::UnregisteredRestore::Refuse
    );

    state.runs.get_mut(&run_id).unwrap().adopted = false;
    assert_eq!(
        unregistered_restore_for(state.runs.get(&run_id).unwrap()),
        crate::worktree::UnregisteredRestore::Write(crate::worktree::BranchTeardown::DeletesBranch)
    );
}
