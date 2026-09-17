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

/// Mint an unbound worktree and hand back (project_id, worktree_id, path).
fn bare_worktree(state: &mut AppState, name: &str) -> (String, String, PathBuf) {
    let project_id = state.project_at(0).id.clone();
    let created = state.handle(req(
        "worktree.create",
        json!({ "project_id": project_id, "name": name }),
    ));
    assert_eq!(created["ok"], true, "{created:?}");
    let result = &created["result"];
    (
        project_id,
        result["worktree_id"].as_str().unwrap().to_string(),
        PathBuf::from(result["path"].as_str().unwrap()),
    )
}

/// The whole git GUI — status, staging, commit, history — works on a
/// worktree the same way it does on the primary checkout, and reads the
/// worktree's own tree rather than the project's.
#[test]
fn the_git_gui_scopes_to_an_external_worktree() {
    let (dir, repo) = init_repo();
    let mut state = git_gui_state(&dir, &repo);
    let (project_id, worktree_id, path) = bare_worktree(&mut state, "scratch");
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
    let (project_id, worktree_id, path) = bare_worktree(&mut state, "scratch");

    let checked_out = state.handle(req(
        "git.checkout",
        json!({ "project_id": project_id, "worktree_id": worktree_id, "branch": "side-quest", "create": true }),
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

/// The rail's worktree affordance (the FAB's smaller sibling): mint a
/// worktree with NO run, no agent and no session — a directory the human
/// then opens a terminal or an agent tab in. It is unbound, so the scan
/// reports it exactly like a worktree made by hand, and the usual
/// adopt-on-first-mutation path still applies.
#[test]
fn worktree_create_mints_an_unbound_worktree_the_scan_can_see() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();

    let created = state.handle(req(
        "worktree.create",
        json!({ "project_id": project_id, "name": "scratch" }),
    ));
    assert_eq!(created["ok"], true, "{created:?}");
    let result = &created["result"];
    let worktree_id = result["worktree_id"].as_str().unwrap().to_string();
    assert!(
        result["branch"].as_str().unwrap().starts_with("build/"),
        "{result:?}"
    );
    assert!(std::path::Path::new(result["path"].as_str().unwrap()).is_dir());
    assert_eq!(result["project_id"], json!(project_id));

    // Nothing was dispatched: no run, no session, no task lifecycle.
    assert!(state.runs.is_empty(), "a bare worktree is not a run");

    // The scan sees it under the id the create returned, so the client can
    // navigate straight to its surface.
    let listed = state.scan_external_worktrees_now(&project_id).unwrap();
    assert!(
        listed.iter().any(|w| w.id == worktree_id),
        "{worktree_id} missing from {listed:?}"
    );

    // A second one does not collide with the first.
    let second = state.handle(req(
        "worktree.create",
        json!({ "project_id": project_id, "name": "scratch" }),
    ));
    assert_eq!(second["ok"], true, "{second:?}");
    assert_ne!(second["result"]["branch"], result["branch"]);
    assert_ne!(second["result"]["worktree_id"], result["worktree_id"]);
}

/// The name the human typed decides the directory and the branch, through the
/// same slugifier every other branch name goes through — it is UNTRUSTED text
/// on its way to a path and a `git` argv.
#[test]
fn worktree_create_names_the_branch_after_the_name() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();

    let created = state.handle(req(
        "worktree.create",
        json!({ "project_id": project_id, "name": "Mascot Model Spike!" }),
    ));
    assert_eq!(created["ok"], true, "{created:?}");
    assert_eq!(created["result"]["branch"], "build/mascot-model-spike");
    assert!(created["result"]["path"]
        .as_str()
        .unwrap()
        .ends_with("mascot-model-spike"));

    // The same name twice cannot collide on disk or on a ref.
    let again = state.handle(req(
        "worktree.create",
        json!({ "project_id": project_id, "name": "Mascot Model Spike!" }),
    ));
    assert_eq!(again["ok"], true, "{again:?}");
    assert_ne!(again["result"]["branch"], created["result"]["branch"]);
}

/// A name that slugifies to nothing would silently become some fallback word,
/// so it is refused instead — the human named it, and the name has to survive.
#[test]
fn worktree_create_requires_a_usable_name() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();

    for name in ["", "   ", "***", "!!!"] {
        let res = state.handle(req(
            "worktree.create",
            json!({ "project_id": project_id, "name": name }),
        ));
        assert_eq!(res["ok"], false, "{name:?} -> {res:?}");
    }
    assert!(
        state.handle(req("worktree.create", json!({ "project_id": project_id })))["ok"] == false
    );
}

/// The create modal's other half: a branch that already exists is checked
/// out into a Build-managed worktree, with nothing cut and no commit of
/// its lost. The branch is somebody's work; Build is only borrowing it a
/// directory.
#[test]
fn worktree_create_checks_out_an_existing_local_branch_without_cutting() {
    let (dir, repo) = init_repo();
    git_in(&repo, &["checkout", "-q", "-b", "theirs"]);
    std::fs::write(repo.join("theirs.txt"), "their work\n").unwrap();
    git_in(&repo, &["add", "."]);
    git_in(&repo, &["commit", "-m", "their work"]);
    git_in(&repo, &["checkout", "-q", "main"]);
    let tip = git2::Repository::open(&repo)
        .unwrap()
        .find_branch("theirs", git2::BranchType::Local)
        .unwrap()
        .get()
        .target()
        .unwrap();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();

    let created = state.handle(req(
        "worktree.create",
        json!({ "project_id": project_id, "branch": "theirs" }),
    ));
    assert_eq!(created["ok"], true, "{created:?}");
    let result = &created["result"];
    assert_eq!(result["branch"], "theirs", "{result:?}");
    assert_eq!(result["branch_was_cut"], false, "{result:?}");
    let path = std::path::PathBuf::from(result["path"].as_str().unwrap());
    assert_eq!(
        std::fs::read_to_string(path.join("theirs.txt")).unwrap(),
        "their work\n",
        "the checkout carries the branch's own work"
    );
    assert_eq!(
        git2::Repository::open(&repo)
            .unwrap()
            .find_branch("theirs", git2::BranchType::Local)
            .unwrap()
            .get()
            .target()
            .unwrap(),
        tip,
        "the branch itself was not moved"
    );
    assert!(state.runs.is_empty(), "a checkout is not a run");
    let listed = state.external_worktrees(&project_id).worktrees;
    assert!(listed
        .iter()
        .any(|w| w.id == result["worktree_id"].as_str().unwrap()));
}

/// A branch only a remote carries is fetched and made local with its
/// upstream set — the bug this affordance exists to fix was cutting an
/// empty branch of the same name over the top of the team's work.
#[test]
fn worktree_create_fetches_a_branch_only_a_remote_carries() {
    let (dir, _repo, origin) = init_repo_with_origin();
    let clone = origin_with_pushed_branch(&dir, &origin, "feature-x");
    let mut state = qa_state(&clone, dir.path());
    let project_id = state.project_at(0).id.clone();

    let created = state.handle(req(
        "worktree.create",
        json!({ "project_id": project_id, "branch": "feature-x" }),
    ));
    assert_eq!(created["ok"], true, "{created:?}");
    let result = &created["result"];
    assert_eq!(result["branch"], "feature-x", "{result:?}");
    assert_eq!(result["branch_was_cut"], false, "{result:?}");
    let path = std::path::PathBuf::from(result["path"].as_str().unwrap());
    assert!(
        path.join("work.rs").is_file(),
        "the remote work came with it"
    );
    let config = git2::Repository::open(&clone).unwrap().config().unwrap();
    assert_eq!(
        config.get_string("branch.feature-x.remote").unwrap(),
        "origin"
    );
    assert_eq!(
        config.get_string("branch.feature-x.merge").unwrap(),
        "refs/heads/feature-x"
    );
}

/// Git refuses to check one branch out twice, and the raw refusal tells
/// the user nothing they can act on. Every checkout that could be holding
/// it is named instead — the run to open, the worktree to adopt, or the
/// repository's own checkout.
#[test]
fn worktree_create_names_the_checkout_already_holding_a_branch() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    let run_id = adopted_run(&mut state, &repo, dir.path(), "run-owned");
    add_external_worktree(&repo, dir.path(), "by-hand", "by-hand");
    let external_worktree_id = external_id(&mut state, &project_id, Some("by-hand"));
    let primary_id = crate::worktree::repository_branch_holder(&repo)
        .unwrap()
        .expect("the repository is on a branch")
        .0;

    for (branch, holder) in [
        ("run-owned", run_id.as_str()),
        ("by-hand", external_worktree_id.as_str()),
        ("main", primary_id.as_str()),
    ] {
        let refused = state.handle(req(
            "worktree.create",
            json!({ "project_id": project_id, "branch": branch }),
        ));
        assert_eq!(refused["ok"], false, "{branch}: {refused:?}");
        let error = refused["error"].as_str().unwrap();
        assert!(error.contains(branch), "{branch}: {error}");
        assert!(
            error.contains(holder),
            "{branch}: {error} names no checkout to act on"
        );
    }
}

/// Naming a branch means that branch. A name nothing anywhere holds is a
/// mistake to be told about, never a fresh empty branch wearing it —
/// cutting one from base is what `name` is for.
#[test]
fn worktree_create_refuses_a_branch_no_ref_holds() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();

    let refused = state.handle(req(
        "worktree.create",
        json!({ "project_id": project_id, "branch": "nobody-cut-this" }),
    ));
    assert_eq!(refused["ok"], false, "{refused:?}");
    assert!(git2::Repository::open(&repo)
        .unwrap()
        .find_branch("nobody-cut-this", git2::BranchType::Local)
        .is_err());

    for spelling in ["HEAD", "-dashed", "not a ref name", ""] {
        let refused = state.handle(req(
            "worktree.create",
            json!({ "project_id": project_id, "branch": spelling }),
        ));
        assert_eq!(refused["ok"], false, "{spelling:?}: {refused:?}");
    }
}

/// The two slots mean opposite things — a branch that exists, or words to
/// cut a new one after — so a call that gives both has said nothing.
#[test]
fn worktree_create_takes_exactly_one_of_branch_and_name() {
    let (dir, repo) = init_repo();
    git_in(&repo, &["branch", "theirs"]);
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();

    let refused = state.handle(req(
        "worktree.create",
        json!({ "project_id": project_id, "branch": "theirs", "name": "theirs" }),
    ));
    assert_eq!(refused["ok"], false, "{refused:?}");

    let refused = state.handle(req("worktree.create", json!({ "project_id": project_id })));
    assert_eq!(refused["ok"], false, "{refused:?}");
    let message = refused["error"].as_str().unwrap();
    assert!(
        message.contains("branch") && message.contains("name"),
        "the refusal names both slots: {message}"
    );
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

/// The same rule at the call site that acts on it: an adopted run's
/// checkout may be one Build only borrowed, so with its registration gone
/// recovery stops instead of re-adding it under a guess that would hand
/// somebody's branch to the next teardown.
#[test]
fn recovering_an_adopted_checkout_whose_registration_is_gone_refuses() {
    let (dir, repo) = init_repo();
    git_in(&repo, &["branch", "theirs"]);
    // Adoption records the checkout's canonical path, and the managed-root
    // guard compares it to the configured root, so the test's root is the
    // canonical one a real install has.
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let mut state = qa_state(&repo, &root);
    let project_id = state.project_at(0).id.clone();
    let created = state.handle(req(
        "worktree.create",
        json!({ "project_id": project_id, "branch": "theirs" }),
    ));
    assert_eq!(created["ok"], true, "{created:?}");
    let adopted = state.handle(req(
        "run.adopt",
        json!({
            "project_id": project_id,
            "worktree_id": created["result"]["worktree_id"],
        }),
    ));
    assert_eq!(adopted["ok"], true, "{adopted:?}");
    let run_id = run_id_of(&adopted);
    let worktree = state.runs[&run_id].worktree.clone();
    let head_sha = git2::Repository::open(&repo)
        .unwrap()
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string();
    git_in(
        &repo,
        &[
            "worktree",
            "remove",
            "--force",
            "--",
            worktree.path.to_str().unwrap(),
        ],
    );
    let recovery_id = "recovery-adopted".to_string();
    state.runs.get_mut(&run_id).unwrap().recovery = Some(crate::run::RecoveryAttempt {
        id: recovery_id.clone(),
        requested_stage_id: String::new(),
        branch: worktree.recorded_branch.clone(),
        state: crate::run::RecoveryState::Started,
        report: None,
        started_at: now_rfc3339(),
        completed_at: None,
    });

    state.on_agent_done(
        &run_id,
        DoneReport {
            phase: DonePhase::Recover,
            status: DoneStatus::Completed,
            summary: "exact branch recovered".into(),
            outputs: DoneOutputs {
                recovery: Some(crate::mcp::RecoveryReport {
                    recovery_id,
                    recovered: true,
                    branch: worktree.recorded_branch.clone(),
                    head_sha,
                    findings: "the branch is still here".into(),
                }),
                ..DoneOutputs::default()
            },
        },
    );

    let active = &state.runs[&run_id];
    assert_eq!(
        active.recovery.as_ref().unwrap().state,
        crate::run::RecoveryState::Failed
    );
    let error = active.last_error.clone().unwrap_or_default();
    assert!(error.contains("registration is gone"), "{error}");
    assert!(!worktree.path.exists(), "nothing was re-added");
    assert!(git2::Repository::open(&repo)
        .unwrap()
        .find_worktree(&worktree.name)
        .is_err());
    assert!(
        git2::Repository::open(&repo)
            .unwrap()
            .find_branch("theirs", git2::BranchType::Local)
            .is_ok(),
        "the branch it borrowed is untouched"
    );
}

#[test]
fn worktree_create_rejects_an_unknown_project() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let res = state.handle(req(
        "worktree.create",
        json!({ "project_id": "proj-nope", "name": "scratch" }),
    ));
    assert_eq!(res["ok"], false, "{res:?}");
}
