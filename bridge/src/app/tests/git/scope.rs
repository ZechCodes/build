use super::*;
use crate::app::{MOVED_GIT_SOURCE, NOT_GIT_SOURCE};

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

// ---- git scope keyed on one original project source -----------------------

/// A project over its original sources: the primary repository, a clone of
/// it at the same commit — identical paths and an identical raw status — and
/// a plain folder.
struct ProjectSources {
    dir: tempfile::TempDir,
    state: AppState,
    project_id: String,
    primary: PathBuf,
    second: PathBuf,
}

/// A clone at the same commit with no remote, so its status is exactly the
/// status of the repository it was cloned from.
fn clone_repo(dir: &std::path::Path, from: &std::path::Path, name: &str) -> PathBuf {
    git_in(dir, &["clone", "-q", from.to_str().unwrap(), name]);
    let clone = std::fs::canonicalize(dir.join(name)).unwrap();
    git_in(&clone, &["remote", "remove", "origin"]);
    crate::git_fixture::configure_repo(&clone);
    clone
}

fn project_sources() -> ProjectSources {
    let (dir, primary) = init_repo();
    let primary = std::fs::canonicalize(primary).unwrap();
    let second = clone_repo(dir.path(), &primary, "second");
    let plain = dir.path().join("plain");
    std::fs::create_dir(&plain).unwrap();
    std::fs::write(plain.join("notes.md"), "plain\n").unwrap();
    let mut state = git_gui_state(&dir, &primary);
    let project_id = state.project_at(0).id.clone();
    let mut sources = state.project_at(0).sources.clone();
    for (id, name, path, is_git) in [
        ("source-2", "Second", second.clone(), true),
        (
            "source-3",
            "Plain",
            std::fs::canonicalize(&plain).unwrap(),
            false,
        ),
    ] {
        sources.push(crate::app::projects::ProjectSource::added(
            id.into(),
            name.into(),
            name.to_lowercase(),
            path,
            is_git,
            "main".into(),
        ));
    }
    assert!(state.projects.set_sources(&project_id, sources));
    ProjectSources {
        dir,
        state,
        project_id,
        primary,
        second,
    }
}

impl ProjectSources {
    /// The scope a client holding this source's row sends: its id and the
    /// path the row carried when the client opened it.
    fn scope(&self, source_id: &str) -> Value {
        let path = self
            .state
            .projects
            .get(&self.project_id)
            .unwrap()
            .sources
            .iter()
            .find(|source| source.id == source_id)
            .map(|source| source.path.display().to_string())
            .unwrap_or_default();
        json!({ "project_id": self.project_id, "source_id": source_id, "source_path": path })
    }

    fn call(&mut self, method: &str, scope: &Value, extra: Value) -> Value {
        let mut params = scope.clone();
        params
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        self.state.handle(req(method, params))
    }
}

/// The same untracked file in the primary and its status-identical clone.
fn same_edit_in_both() -> (ProjectSources, Value, Value) {
    let sources = project_sources();
    for repo in [&sources.primary, &sources.second] {
        std::fs::write(repo.join("same.txt"), "same\n").unwrap();
    }
    let (first, second) = (sources.scope("source-1"), sources.scope("source-2"));
    (sources, first, second)
}

/// Status and diff reach the source they name — including two repositories
/// whose paths and raw status are the same, which never share a cache key —
/// while legacy `{project_id}` keeps reaching the primary checkout with its
/// established keys.
#[test]
fn project_source_git_addresses_each_source_with_its_own_keys() {
    let (mut sources, first, second) = same_edit_in_both();
    let first_status = sources.call("git.status", &first, json!({}));
    let second_status = sources.call("git.status", &second, json!({}));
    assert_eq!(first_status["ok"], true, "{first_status:?}");
    assert_eq!(second_status["ok"], true, "{second_status:?}");
    assert_eq!(
        first_status["result"]["path"],
        sources.primary.display().to_string()
    );
    assert_eq!(
        second_status["result"]["path"],
        sources.second.display().to_string()
    );
    let legacy = sources.state.handle(req(
        "git.status",
        json!({ "project_id": sources.project_id }),
    ));
    // Legacy and source-1 read the same repository at the same moment, so
    // their raw status is identical: only the scope can tell their keys apart.
    assert_eq!(legacy["result"]["path"], first_status["result"]["path"]);
    let keys = [
        &first_status["result"]["status_key"],
        &second_status["result"]["status_key"],
        &legacy["result"]["status_key"],
    ];
    assert_ne!(keys[0], keys[1], "two sources never share a status key");
    assert_ne!(keys[0], keys[2], "a source scope is its own cache record");
    let unchanged = sources.call(
        "git.status",
        &second,
        json!({ "if_status_key": second_status["result"]["status_key"] }),
    );
    assert_eq!(unchanged["result"]["unchanged"], true, "{unchanged:?}");
    let first_diff = sources.call("git.diff", &first, json!({ "paths": ["same.txt"] }));
    let second_diff = sources.call("git.diff", &second, json!({ "paths": ["same.txt"] }));
    assert_ne!(
        first_diff["result"]["files"][0]["content_key"],
        second_diff["result"]["files"][0]["content_key"],
    );
}

/// Mutations, history, refs and changesets each stay in the source they name.
#[test]
fn project_source_git_commits_one_source_and_reads_its_own_history() {
    let (mut sources, first, second) = same_edit_in_both();
    let staged = sources.call("git.stage", &second, json!({ "paths": ["same.txt"] }));
    assert_eq!(staged["ok"], true, "{staged:?}");
    let committed = sources.call("git.commit", &second, json!({ "message": "second only" }));
    assert_eq!(committed["ok"], true, "{committed:?}");
    let second_after = sources.call("git.status", &second, json!({}));
    assert!(!has_file_entry(&second_after["result"], "same.txt"));
    let first_after = sources.call("git.status", &first, json!({}));
    assert!(has_file_entry(&first_after["result"], "same.txt"));

    for (scope, subject) in [(&second, "second only"), (&first, "initial")] {
        let log = sources.call("git.log", scope, json!({}));
        let commits = log["result"]["commits"].as_array().unwrap();
        assert_eq!(commits[0]["subject"], subject, "{log:?}");
        assert!(
            commits
                .iter()
                .all(|commit| commit.get("ahead_of_base").is_none()
                    && commit.get("unpushed").is_none()),
            "a project source's history marks nothing, like the primary's: {log:?}"
        );
    }
    let refs = sources.call("git.refs", &second, json!({}));
    assert_eq!(refs["ok"], true, "{refs:?}");
    // Uncommitted only in the primary: each source's changeset is its own.
    let changesets = [&first, &second].map(|scope| {
        sources.call(
            "git.changeset_diff",
            scope,
            json!({ "paths": ["same.txt"] }),
        )
    });
    assert_eq!(
        changesets[0]["result"]["files"][0]["path"], "same.txt",
        "{changesets:?}"
    );
    assert_eq!(
        changesets[1]["result"]["files"],
        json!([]),
        "{changesets:?}"
    );
}

/// A plain folder is a source with Files and terminals but no Git: a Git verb
/// naming it says so instead of reaching some enclosing repository.
#[test]
fn project_source_git_refuses_a_plain_folder() {
    let mut sources = project_sources();
    let plain = sources.scope("source-3");
    for (method, extra) in [
        ("git.status", json!({})),
        ("git.log", json!({})),
        ("git.stage", json!({ "paths": ["notes.md"] })),
    ] {
        let refused = sources.call(method, &plain, extra);
        assert_eq!(refused["error_code"], "invalid_params", "{refused:?}");
        assert_eq!(refused["error"], NOT_GIT_SOURCE, "{refused:?}");
    }
}

/// The scope names exactly one registered source by id and asserts where the
/// caller opened it. Anything else is refused before Git runs, and a source
/// moved after the caller opened it never receives the caller's commit.
#[test]
fn project_source_git_refuses_mixed_or_moved_scope() {
    let mut sources = project_sources();
    let second = sources.scope("source-2");
    let (dir, project_id) = (sources.dir.path().to_path_buf(), sources.project_id.clone());
    let other_repo = init_repo_named(&dir, "other");
    let other_id = sources.state.add_project(other_repo, "main".into());
    let mut refusals = vec![
        (
            json!({ "project_id": project_id, "source_id": "source-2" }),
            "invalid_params",
        ),
        (
            json!({ "project_id": project_id, "source_id": "", "source_path": second["source_path"] }),
            "invalid_params",
        ),
        (
            json!({ "project_id": project_id, "source_path": second["source_path"] }),
            "invalid_params",
        ),
        (
            json!({ "project_id": project_id, "source_id": "missing", "source_path": "/nowhere" }),
            "not_found",
        ),
        (
            json!({ "project_id": "missing", "source_id": "source-2", "source_path": second["source_path"] }),
            "not_found",
        ),
        (
            json!({ "project_id": other_id, "source_id": "source-2", "source_path": second["source_path"] }),
            "not_found",
        ),
        (
            json!({ "project_id": project_id, "source_id": "source-2", "source_path": dir.join("elsewhere") }),
            "conflict",
        ),
    ];
    for field in ["workspace_id", "run_id", "worktree_id"] {
        let mut mixed = second.clone();
        mixed[field] = json!("other-scope");
        refusals.push((mixed, "invalid_params"));
    }
    for (scope, code) in refusals {
        let refused = sources.call("git.status", &scope, json!({}));
        assert_eq!(refused["error_code"], code, "{scope}: {refused:?}");
    }

    std::fs::write(sources.second.join("kept.txt"), "kept\n").unwrap();
    let moved = clone_repo(&dir, &sources.second, "moved");
    std::fs::write(moved.join("kept.txt"), "kept\n").unwrap();
    let update = sources.state.handle(req(
        "project.update_source",
        json!({ "project_id": project_id, "source_id": "source-2", "path": moved }),
    ));
    assert_eq!(update["ok"], true, "{update:?}");
    for (method, extra) in [
        ("git.status", json!({})),
        ("git.stage", json!({ "paths": ["kept.txt"] })),
        ("git.commit", json!({ "message": "redirected" })),
        ("git.discard", json!({ "paths": ["kept.txt"] })),
    ] {
        let refused = sources.call(method, &second, extra);
        assert_eq!(refused["error_code"], "conflict", "{method}: {refused:?}");
        assert_eq!(refused["error"], MOVED_GIT_SOURCE, "{method}: {refused:?}");
    }
    for repo in [&sources.second, &moved] {
        let status = crate::gitgui::status_payload(repo).unwrap();
        assert!(has_file_entry(&status, "kept.txt"), "{status:?}");
    }

    let fresh = sources.scope("source-2");
    assert_ne!(fresh["source_path"], second["source_path"]);
    let status = sources.call("git.status", &fresh, json!({}));
    assert_eq!(status["result"]["path"], moved.display().to_string());
}

/// A repository with no commit yet stages and makes its first commit, and its
/// history is empty until then rather than invented.
#[test]
fn project_source_git_serves_an_unborn_repository() {
    let mut sources = project_sources();
    let unborn = sources.dir.path().join("unborn");
    std::fs::create_dir(&unborn).unwrap();
    git_in(&unborn, &["init", "-q", "-b", "main"]);
    crate::git_fixture::configure_repo(&unborn);
    let mut rows = sources.state.project_at(0).sources.clone();
    rows.push(crate::app::projects::ProjectSource::added(
        "source-4".into(),
        "Unborn".into(),
        "unborn".into(),
        std::fs::canonicalize(&unborn).unwrap(),
        true,
        "main".into(),
    ));
    assert!(sources
        .state
        .projects
        .set_sources(&sources.project_id, rows));
    let scope = sources.scope("source-4");

    let empty = sources.call("git.log", &scope, json!({}));
    assert_eq!(empty["ok"], true, "{empty:?}");
    assert_eq!(empty["result"]["commits"], json!([]), "{empty:?}");
    std::fs::write(unborn.join("first.txt"), "first\n").unwrap();
    let staged = sources.call("git.stage", &scope, json!({ "paths": ["first.txt"] }));
    assert_eq!(staged["ok"], true, "{staged:?}");
    let committed = sources.call("git.commit", &scope, json!({ "message": "first" }));
    assert_eq!(committed["ok"], true, "{committed:?}");
    let log = sources.call("git.log", &scope, json!({}));
    assert_eq!(
        log["result"]["commits"].as_array().unwrap().len(),
        1,
        "{log:?}"
    );
}

#[test]
fn review_550_equivalent_source_spellings_preserve_scope() {
    let mut sources = project_sources();
    let scope = sources.scope("source-2");
    let alias = sources.dir.path().join("second-alias");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&sources.second, &alias).unwrap();
    let spellings = [
        sources.second.join(".").display().to_string(),
        format!("{}/", sources.second.display()),
        alias.display().to_string(),
    ];
    for spelling in spellings {
        let update = sources.state.handle(req(
            "project.update_source",
            json!({"project_id": sources.project_id, "source_id": "source-2", "path": spelling}),
        ));
        assert_eq!(update["ok"], true, "{update}");
        assert_eq!(sources.scope("source-2"), scope);
        assert_eq!(sources.call("git.status", &scope, json!({}))["ok"], true);
    }
}

#[test]
fn review_550_changeset_content_keys_are_source_qualified() {
    let (mut sources, first, second) = same_edit_in_both();
    let first_diff = sources.call("git.changeset_diff", &first, json!({"paths": ["same.txt"]}));
    let second_diff = sources.call(
        "git.changeset_diff",
        &second,
        json!({"paths": ["same.txt"]}),
    );
    assert_eq!(first_diff["ok"], true, "{first_diff}");
    assert_eq!(second_diff["ok"], true, "{second_diff}");
    let a = &first_diff["result"]["files"][0]["content_key"];
    let b = &second_diff["result"]["files"][0]["content_key"];
    assert!(a.is_string() && b.is_string());
    assert_ne!(a, b, "source changeset keys must include source identity");
}

/// `git.unpushed` answers a project source's keys in its namespace too: the
/// same unpublished edit in two sources shares neither a `diff_key` nor a
/// `content_key`, and a source's own key still reads as unchanged (#550).
#[test]
fn project_source_unpushed_keys_are_source_qualified() {
    let (mut sources, first, second) = same_edit_in_both();
    let [first_read, second_read] = [&first, &second].map(|scope| {
        let read = sources.call("git.unpushed", scope, json!({ "patch": false }));
        assert_eq!(read["ok"], true, "{read:?}");
        read["result"].clone()
    });
    assert!(first_read["diff_key"].is_string(), "{first_read:?}");
    assert_ne!(first_read["diff_key"], second_read["diff_key"]);
    let content_key = |read: &Value| read["files"][0]["content_key"].clone();
    assert!(content_key(&first_read).is_string(), "{first_read:?}");
    assert_ne!(content_key(&first_read), content_key(&second_read));
    let held = sources.call(
        "git.unpushed",
        &second,
        json!({ "if_diff_key": second_read["diff_key"] }),
    );
    assert_eq!(
        held["result"],
        json!({ "unchanged": true, "diff_key": second_read["diff_key"] }),
        "{held:?}"
    );
}

#[test]
fn review_550_same_root_aliases_keep_distinct_status_keys() {
    let mut sources = project_sources();
    let mut rows = sources.state.project_at(0).sources.clone();
    rows[1].path = sources.primary.clone();
    assert!(sources
        .state
        .projects
        .set_sources(&sources.project_id, rows));
    let first = sources.scope("source-1");
    let second = sources.scope("source-2");
    let a = sources.call("git.status", &first, json!({}));
    let b = sources.call("git.status", &second, json!({}));
    assert_eq!(a["ok"], true);
    assert_eq!(b["ok"], true);
    assert_eq!(a["result"]["path"], b["result"]["path"]);
    assert_ne!(a["result"]["status_key"], b["result"]["status_key"]);
}

#[test]
fn review_550_malformed_selectors_refuse() {
    let mut sources = project_sources();
    let second = sources.scope("source-2");
    for field in ["project_id", "source_id", "source_path"] {
        for value in [json!(""), json!(false), json!(17), json!([]), json!({})] {
            let mut bad = second.clone();
            bad[field] = value;
            let reply = sources.call("git.status", &bad, json!({}));
            assert_eq!(reply["error_code"], "invalid_params", "{bad}: {reply}");
        }
    }
}
