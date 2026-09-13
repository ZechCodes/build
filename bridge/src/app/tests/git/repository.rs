use super::*;

/// Run git without asserting success — for setting up conflict/rebase
/// states whose whole point is a non-zero exit.
fn git_try(dir: &std::path::Path, args: &[&str]) {
    let _ = Command::new("git").args(args).current_dir(dir).status();
}

/// A working repo wired to a bare "origin" it already tracks (main →
/// origin/main, ahead 0 / behind 0).
pub(in crate::app::tests) fn init_repo_with_origin() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let (dir, repo) = init_repo();
    let origin = dir.path().join("origin.git");
    git_in(
        dir.path(),
        &[
            "clone",
            "--bare",
            repo.to_str().unwrap(),
            origin.to_str().unwrap(),
        ],
    );
    git_in(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git_in(&repo, &["fetch", "origin"]);
    git_in(&repo, &["branch", "--set-upstream-to=origin/main", "main"]);
    (dir, repo, origin)
}

/// A second working checkout of `origin`, standing in for another dev.
fn clone_working(origin: &std::path::Path, dest: &std::path::Path) {
    git_in(
        dest.parent().unwrap(),
        &["clone", origin.to_str().unwrap(), dest.to_str().unwrap()],
    );
    git_in(dest, &["config", "user.email", "o@build.ing"]);
    git_in(dest, &["config", "user.name", "O"]);
}

/// Another dev pushes one commit on `branch` to `origin`; the path returned
/// is a fresh clone that has fetched it but never checked it out, so the
/// branch exists there only as `origin/<branch>`.
pub(in crate::app::tests) fn origin_with_pushed_branch(
    dir: &tempfile::TempDir,
    origin: &std::path::Path,
    branch: &str,
) -> std::path::PathBuf {
    let other = dir.path().join("other");
    clone_working(origin, &other);
    git_in(&other, &["checkout", "-b", branch]);
    std::fs::write(other.join("work.rs"), "one\n").unwrap();
    git_in(&other, &["add", "."]);
    git_in(&other, &["commit", "-m", "remote work"]);
    git_in(&other, &["push", "origin", branch]);
    let clone = dir.path().join("clone");
    clone_working(origin, &clone);
    clone
}

#[test]
fn git_status_carries_the_repo_management_fields() {
    let (dir, repo) = init_repo();
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();

    let res = state.handle(req("git.status", json!({ "project_id": project_id })));
    assert_eq!(res["ok"], true, "{res:?}");
    let result = &res["result"];
    assert_eq!(result["repo_state"], "clean");
    // No remote configured → upstream/ahead/behind are null, not NaN.
    assert!(result["upstream"].is_null());
    assert!(result["ahead"].is_null());
    assert!(result["behind"].is_null());
    assert_eq!(result["stash_count"], 0);
}

#[test]
fn git_fetch_pull_push_round_trip_through_a_bare_origin() {
    let (dir, repo, origin) = init_repo_with_origin();
    let other = dir.path().join("other");
    clone_working(&origin, &other);
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();

    // Another dev pushes a commit to origin.
    std::fs::write(other.join("remote.txt"), "remote\n").unwrap();
    git_in(&other, &["add", "remote.txt"]);
    git_in(&other, &["commit", "-m", "remote work"]);
    git_in(&other, &["push", "origin", "main"]);

    // git.fetch updates the tracking ref: we are now behind by one.
    let fetched = state.handle(req("git.fetch", json!({ "project_id": project_id })));
    assert_eq!(fetched["ok"], true, "{fetched:?}");
    assert_eq!(fetched["result"]["upstream"], "origin/main");
    assert_eq!(fetched["result"]["behind"], 1);
    assert_eq!(fetched["result"]["ahead"], 0);

    // git.pull (ff) fast-forwards the branch onto the remote commit.
    let pulled = state.handle(req("git.pull", json!({ "project_id": project_id })));
    assert_eq!(pulled["ok"], true, "{pulled:?}");
    assert_eq!(pulled["result"]["behind"], 0);
    assert!(repo.join("remote.txt").exists());

    // A local commit, then git.push publishes it to origin.
    std::fs::write(repo.join("local.txt"), "local\n").unwrap();
    git_in(&repo, &["add", "local.txt"]);
    git_in(&repo, &["commit", "-m", "local work"]);
    let ahead = state.handle(req("git.status", json!({ "project_id": project_id })));
    assert_eq!(ahead["result"]["ahead"], 1);

    let pushed = state.handle(req("git.push", json!({ "project_id": project_id })));
    assert_eq!(pushed["ok"], true, "{pushed:?}");
    assert_eq!(pushed["result"]["ahead"], 0);
    assert_eq!(pushed["result"]["behind"], 0);

    // The other checkout can now fetch our commit — proof it reached origin.
    git_in(&other, &["fetch", "origin"]);
    let log = Command::new("git")
        .args(["log", "--oneline", "origin/main"])
        .current_dir(&other)
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&log.stdout).contains("local work"));
}

#[test]
fn git_push_sets_the_upstream_on_the_first_push() {
    let (dir, repo) = init_repo();
    let origin = dir.path().join("origin.git");
    git_in(
        dir.path(),
        &["init", "--bare", "-b", "main", origin.to_str().unwrap()],
    );
    git_in(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();

    // No upstream yet.
    let before = state.handle(req("git.status", json!({ "project_id": project_id })));
    assert!(before["result"]["upstream"].is_null());

    let pushed = state.handle(req("git.push", json!({ "project_id": project_id })));
    assert_eq!(pushed["ok"], true, "{pushed:?}");
    assert_eq!(pushed["result"]["upstream"], "origin/main");
    assert_eq!(pushed["result"]["ahead"], 0);
}

#[test]
fn git_push_force_uses_force_with_lease_after_a_rewrite() {
    let (dir, repo, _origin) = init_repo_with_origin();
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();

    // Publish a commit, then rewrite it so local diverges from origin.
    std::fs::write(repo.join("x.txt"), "one\n").unwrap();
    git_in(&repo, &["add", "x.txt"]);
    git_in(&repo, &["commit", "-m", "first"]);
    assert_eq!(
        state.handle(req("git.push", json!({ "project_id": project_id })))["ok"],
        true
    );
    std::fs::write(repo.join("x.txt"), "two\n").unwrap();
    git_in(&repo, &["commit", "-a", "--amend", "-m", "rewritten"]);

    // A plain push is rejected (non-fast-forward); force-with-lease wins.
    let plain = state.handle(req("git.push", json!({ "project_id": project_id })));
    assert_eq!(plain["ok"], false, "{plain:?}");
    let forced = state.handle(req(
        "git.push",
        json!({ "project_id": project_id, "force": true }),
    ));
    assert_eq!(forced["ok"], true, "{forced:?}");
}

#[test]
fn git_push_refuses_a_detached_head() {
    let (dir, repo, _origin) = init_repo_with_origin();
    git_in(&repo, &["checkout", "--detach", "HEAD"]);
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();

    let res = state.handle(req("git.push", json!({ "project_id": project_id })));
    assert_eq!(res["ok"], false, "{res:?}");
    assert_eq!(res["error"], "cannot push a detached HEAD");
}

#[test]
fn git_pull_ff_only_refuses_divergent_history() {
    let (dir, repo, origin) = init_repo_with_origin();
    let other = dir.path().join("other");
    clone_working(&origin, &other);
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();

    std::fs::write(other.join("theirs.txt"), "theirs\n").unwrap();
    git_in(&other, &["add", "theirs.txt"]);
    git_in(&other, &["commit", "-m", "theirs"]);
    git_in(&other, &["push", "origin", "main"]);

    std::fs::write(repo.join("mine.txt"), "mine\n").unwrap();
    git_in(&repo, &["add", "mine.txt"]);
    git_in(&repo, &["commit", "-m", "mine"]);

    assert_eq!(
        state.handle(req("git.fetch", json!({ "project_id": project_id })))["ok"],
        true
    );
    let pulled = state.handle(req("git.pull", json!({ "project_id": project_id })));
    assert_eq!(pulled["ok"], false, "{pulled:?}");
    assert!(
        pulled["error"].as_str().unwrap().contains("fast-forward")
            || pulled["error"].as_str().unwrap().contains("fast forward"),
        "{pulled:?}"
    );
}

#[test]
fn git_pull_conflict_leaves_a_visible_merging_state() {
    let (dir, repo, origin) = init_repo_with_origin();
    let other = dir.path().join("other");
    clone_working(&origin, &other);
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();

    // Both sides edit README differently; the other side lands first.
    std::fs::write(other.join("README.md"), "# theirs\n").unwrap();
    git_in(&other, &["commit", "-am", "theirs"]);
    git_in(&other, &["push", "origin", "main"]);
    std::fs::write(repo.join("README.md"), "# mine\n").unwrap();
    git_in(&repo, &["commit", "-am", "mine"]);
    assert_eq!(
        state.handle(req("git.fetch", json!({ "project_id": project_id })))["ok"],
        true
    );

    let pulled = state.handle(req(
        "git.pull",
        json!({ "project_id": project_id, "mode": "merge" }),
    ));
    assert_eq!(pulled["ok"], false, "{pulled:?}");

    // The conflict is legible in the very next status: merging + a U file.
    let status = state.handle(req("git.status", json!({ "project_id": project_id })));
    assert_eq!(status["result"]["repo_state"], "merging");
    let readme = file_entry(&status["result"], "README.md");
    assert_eq!(readme["index_status"], "U");
    assert_eq!(readme["worktree_status"], "U");
}

#[test]
fn git_branches_lists_locals_current_first() {
    let (dir, repo) = init_repo();
    git_in(&repo, &["branch", "feature-a"]);
    git_in(&repo, &["branch", "feature-b"]);
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();

    let res = state.handle(req("git.branches", json!({ "project_id": project_id })));
    assert_eq!(res["ok"], true, "{res:?}");
    assert_eq!(res["result"]["current"], "main");
    let branches = res["result"]["branches"].as_array().unwrap();
    assert_eq!(branches.len(), 3);
    // Current branch sorts first.
    assert_eq!(branches[0]["name"], "main");
    assert_eq!(branches[0]["is_current"], true);
    assert_eq!(branches[0]["ahead"], 0);
    assert_eq!(branches[0]["behind"], 0);
    assert!(branches[0]["upstream"].is_null());
    // A branch with nothing on it yet has nothing to weigh.
    assert_eq!(branches[0]["stat"]["insertions"], 0, "{branches:?}");
    assert_eq!(branches[0]["stat"]["deletions"], 0, "{branches:?}");
    assert!(branches.iter().any(|b| b["name"] == "feature-a"));
}

/// The switcher reads a branch's own weight against the project's base —
/// not the checked-out branch's, and not requiring the branch to BE
/// checked out at all.
#[test]
fn git_branches_carries_each_branchs_own_diffstat_against_base() {
    let (dir, repo) = init_repo();
    git_in(&repo, &["checkout", "-b", "feature-a"]);
    std::fs::write(repo.join("feature.rs"), "one\ntwo\nthree\n").unwrap();
    git_in(&repo, &["add", "."]);
    git_in(&repo, &["commit", "-m", "feature work"]);
    git_in(&repo, &["checkout", "main"]);
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();

    let res = state.handle(req("git.branches", json!({ "project_id": project_id })));
    assert_eq!(res["ok"], true, "{res:?}");
    let branches = res["result"]["branches"].as_array().unwrap();
    let feature = branches
        .iter()
        .find(|b| b["name"] == "feature-a")
        .expect("feature-a is listed even though it is not checked out");
    assert_eq!(feature["stat"]["files_changed"], 1, "{feature:?}");
    assert_eq!(feature["stat"]["insertions"], 3, "{feature:?}");
    assert_eq!(feature["stat"]["deletions"], 0, "{feature:?}");
    let main = branches.iter().find(|b| b["name"] == "main").unwrap();
    assert_eq!(main["stat"]["insertions"], 0, "{main:?}");
}

/// A clone is a repository of its own, so a commit made in it is invisible
/// to the project until published. The switcher publishes every held
/// branch through the façade before it reads the project's refs (spec
/// §0.4), so a clone's row weighs what the clone holds; a linked
/// worktree's publish is a no-op, and its row was never stale.
#[test]
fn git_branches_weighs_a_clones_branch_after_publishing_it() {
    let (dir, repo) = init_repo();
    if !crate::isolation::probe::rift_or_skip(dir.path()) {
        return;
    }
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();
    let clone = state
        .orch_for(&project_id)
        .unwrap()
        .worktrees()
        .create("cloned", "main", Isolation::Rift)
        .unwrap()
        .worktree;
    std::fs::write(clone.path.join("cloned.rs"), "one\ntwo\nthree\n").unwrap();
    git_in(&clone.path, &["add", "."]);
    git_in(&clone.path, &["commit", "-m", "work in the clone"]);

    let res = state.handle(req("git.branches", json!({ "project_id": project_id })));
    assert_eq!(res["ok"], true, "{res:?}");
    let branches = res["result"]["branches"].as_array().unwrap();
    let cloned = branches
        .iter()
        .find(|b| b["name"] == clone.branch())
        .expect("the clone's branch is the project's to list");
    assert_eq!(
        cloned["stat"]["insertions"], 3,
        "the clone's commit weighs on its row: {cloned:?}"
    );
}

/// A branch checked out in a worktree Build never adopted is unpickable
/// as a plain checkout — git refuses the same branch in two working
/// directories — so the switcher flags it with the worktree that has it,
/// for the client to adopt instead.
#[test]
fn git_branches_flags_a_branch_checked_out_in_an_unadopted_worktree() {
    let (dir, repo) = init_repo();
    add_external_worktree(&repo, dir.path(), "elsewhere", "feature-elsewhere");
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();
    let worktree_id = state
        .scan_external_worktrees_now(&project_id)
        .unwrap()
        .into_iter()
        .find(|w| w.branch.as_deref() == Some("feature-elsewhere"))
        .expect("discoverable")
        .id;

    let res = state.handle(req("git.branches", json!({ "project_id": project_id })));
    assert_eq!(res["ok"], true, "{res:?}");
    let branches = res["result"]["branches"].as_array().unwrap();
    let elsewhere = branches
        .iter()
        .find(|b| b["name"] == "feature-elsewhere")
        .unwrap();
    assert_eq!(
        elsewhere["holder"],
        json!({ "kind": "external_worktree", "id": worktree_id }),
        "{elsewhere:?}"
    );
    let main = branches.iter().find(|b| b["name"] == "main").unwrap();
    assert_eq!(
        main["holder"]["kind"], "primary_checkout",
        "the checked-out-here branch is the repository's own: {main:?}"
    );
}

/// Adopting the very worktree Build already runs a run in stays unflagged
/// — that branch is a normal checkout target for anything else, not a
/// switcher special case (adoption already happened).
#[test]
fn git_branches_does_not_flag_a_branch_a_run_already_owns() {
    let (dir, repo) = init_repo();
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();
    let run_id = adopted_run(&mut state, &repo, dir.path(), "feature-adopted");
    assert!(state.runs.contains_key(&run_id));

    let res = state.handle(req("git.branches", json!({ "project_id": project_id })));
    assert_eq!(res["ok"], true, "{res:?}");
    let branches = res["result"]["branches"].as_array().unwrap();
    let adopted = branches
        .iter()
        .find(|b| b["name"] == "feature-adopted")
        .unwrap();
    assert_eq!(adopted["holder"]["kind"], "run", "{adopted:?}");
}

#[test]
fn worktree_diff_reports_existing_file_mtimes_and_omits_deletions() {
    let (dir, repo) = init_repo();
    let checkout = add_external_worktree(&repo, dir.path(), "timestamped", "timestamped");
    std::fs::write(checkout.join("new.txt"), "new\n").unwrap();
    std::fs::remove_file(checkout.join("README.md")).unwrap();
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();
    let worktree_id = state
        .scan_external_worktrees_now(&project_id)
        .unwrap()
        .into_iter()
        .find(|worktree| worktree.branch.as_deref() == Some("timestamped"))
        .unwrap()
        .id;

    let result = state.handle(req(
        "worktree.diff",
        json!({ "project_id": project_id, "worktree_id": worktree_id }),
    ));
    assert_eq!(result["ok"], true, "{result:?}");
    let edited_at = result["result"]["file_edited_at"].as_object().unwrap();
    assert!(edited_at["new.txt"].as_u64().unwrap() > 0);
    assert!(edited_at.get("README.md").is_none());

    let diff_key = result["result"]["diff_key"].as_str().unwrap().to_string();
    let unchanged = state.handle(req(
        "worktree.diff",
        json!({
            "project_id": project_id, "worktree_id": worktree_id, "if_diff_key": diff_key,
        }),
    ));
    assert_eq!(
        unchanged["result"],
        json!({ "unchanged": true, "diff_key": diff_key }),
        "{unchanged:?}"
    );

    std::fs::write(checkout.join("new.txt"), "newer\n").unwrap();
    let changed = state.handle(req(
        "worktree.diff",
        json!({
            "project_id": project_id, "worktree_id": worktree_id, "if_diff_key": diff_key,
        }),
    ));
    assert_ne!(changed["result"]["diff_key"], diff_key, "{changed:?}");
    assert!(changed["result"]["patch"]
        .as_str()
        .unwrap()
        .contains("newer"));
}

/// A branch nobody here has ever checked out is still work the user can
/// start: it is listed once, named by the local branch it would become,
/// and it says which remote a fetch would come from. The clone's
/// `origin/HEAD` is a symbolic pointer at another branch, not a branch —
/// it never becomes a row — and `main`, which has both a local ref and a
/// remote-tracking ref, is one row carrying its upstream.
#[test]
fn git_branches_lists_a_remote_only_branch_with_its_remote() {
    let (dir, _repo, origin) = init_repo_with_origin();
    let clone = origin_with_pushed_branch(&dir, &origin, "feature-x");
    let mut state = git_gui_state(&dir, &clone);
    let project_id = state.project_at(0).id.clone();

    let res = state.handle(req("git.branches", json!({ "project_id": project_id })));
    assert_eq!(res["ok"], true, "{res:?}");
    let branches = res["result"]["branches"].as_array().unwrap();

    let feature = branches
        .iter()
        .find(|b| b["name"] == "feature-x")
        .unwrap_or_else(|| panic!("the remote-only branch is offerable: {branches:?}"));
    assert_eq!(feature["remote"], "origin", "{feature:?}");
    assert_eq!(feature["is_current"], false, "{feature:?}");
    assert!(feature["upstream"].is_null(), "{feature:?}");
    assert!(feature["holder"].is_null(), "{feature:?}");
    assert_eq!(feature["stat"]["insertions"], 1, "{feature:?}");

    assert!(
        !branches.iter().any(|b| b["name"] == "HEAD"),
        "a symbolic remote ref is not a branch: {branches:?}"
    );
    let mains: Vec<&Value> = branches.iter().filter(|b| b["name"] == "main").collect();
    assert_eq!(mains.len(), 1, "{branches:?}");
    assert!(mains[0]["remote"].is_null(), "{mains:?}");
    assert_eq!(mains[0]["upstream"], "origin/main", "{mains:?}");
}

/// A remote branch whose name has a slash is one branch named
/// `feature/nested`, not a branch `nested` under some other heading: the
/// name is everything after `refs/remotes/<remote>/`, however many
/// segments that is. Nested names are the common case for a team's
/// branches, so a listing that dropped them would offer the user almost
/// nothing.
#[test]
fn git_branches_lists_a_remote_only_branch_whose_name_has_a_slash() {
    let (dir, _repo, origin) = init_repo_with_origin();
    let clone = origin_with_pushed_branch(&dir, &origin, "feature/nested");
    let mut state = git_gui_state(&dir, &clone);
    let project_id = state.project_at(0).id.clone();

    let res = state.handle(req("git.branches", json!({ "project_id": project_id })));
    assert_eq!(res["ok"], true, "{res:?}");
    let branches = res["result"]["branches"].as_array().unwrap();

    let nested: Vec<&Value> = branches
        .iter()
        .filter(|b| b["name"] == "feature/nested")
        .collect();
    assert_eq!(nested.len(), 1, "{branches:?}");
    assert_eq!(nested[0]["remote"], "origin", "{nested:?}");
    assert!(
        !branches.iter().any(|b| b["name"] == "nested"),
        "the name is the whole suffix, not its last segment: {branches:?}"
    );
}

/// A branch two remotes both carry is still one branch to offer. It is
/// listed once, from the remote a fetch would come from: `origin` when
/// origin has it, whatever the other remote is called — git lists remotes
/// alphabetically, and `fork` sorts before `origin`.
#[test]
fn git_branches_lists_a_branch_two_remotes_carry_once_preferring_origin() {
    let (dir, _repo, origin) = init_repo_with_origin();
    let clone = origin_with_pushed_branch(&dir, &origin, "feature-x");
    git_in(&clone, &["remote", "add", "fork", origin.to_str().unwrap()]);
    git_in(&clone, &["fetch", "fork"]);
    let mut state = git_gui_state(&dir, &clone);
    let project_id = state.project_at(0).id.clone();

    let res = state.handle(req("git.branches", json!({ "project_id": project_id })));
    assert_eq!(res["ok"], true, "{res:?}");
    let branches = res["result"]["branches"].as_array().unwrap();
    let features: Vec<&Value> = branches
        .iter()
        .filter(|b| b["name"] == "feature-x")
        .collect();
    assert_eq!(features.len(), 1, "{branches:?}");
    assert_eq!(features[0]["remote"], "origin", "{features:?}");
    let mains: Vec<&Value> = branches.iter().filter(|b| b["name"] == "main").collect();
    assert_eq!(mains.len(), 1, "{branches:?}");
}

/// A branch Build already runs is not a checkout the picker can offer —
/// it is a run to open — so the row names the run holding it.
#[test]
fn git_branches_names_the_run_that_owns_a_branch() {
    let (dir, repo) = init_repo();
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();
    let run_id = adopted_run(&mut state, &repo, dir.path(), "feature-adopted");

    let res = state.handle(req("git.branches", json!({ "project_id": project_id })));
    assert_eq!(res["ok"], true, "{res:?}");
    let branches = res["result"]["branches"].as_array().unwrap();
    let adopted = branches
        .iter()
        .find(|b| b["name"] == "feature-adopted")
        .unwrap();
    assert_eq!(
        adopted["holder"],
        json!({ "kind": "run", "id": run_id }),
        "{adopted:?}"
    );
    let main = branches.iter().find(|b| b["name"] == "main").unwrap();
    assert_eq!(main["holder"]["kind"], "primary_checkout", "{main:?}");
}

/// The repository's own checkout is deliberately absent from the external
/// scan, so without asking after it the branch it holds would look free to
/// check out a second time — which git refuses. The row names it, with the
/// worktree id `run.adopt` adopts the primary by.
#[test]
fn git_branches_names_the_primary_checkout_holding_a_branch() {
    let (dir, repo) = init_repo();
    git_in(&repo, &["branch", "feature-idle"]);
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();
    let primary_id = state
        .project_at(0)
        .orch
        .worktrees()
        .describe_primary("main")
        .unwrap()
        .id;

    let res = state.handle(req("git.branches", json!({ "project_id": project_id })));
    assert_eq!(res["ok"], true, "{res:?}");
    let branches = res["result"]["branches"].as_array().unwrap();
    let main = branches.iter().find(|b| b["name"] == "main").unwrap();
    assert_eq!(
        main["holder"],
        json!({ "kind": "primary_checkout", "id": primary_id }),
        "{main:?}"
    );
    let idle = branches
        .iter()
        .find(|b| b["name"] == "feature-idle")
        .unwrap();
    assert!(idle["holder"].is_null(), "{idle:?}");
}

/// A run adopted over the primary checkout is two holders of one branch,
/// and only one of them can be pressed: the run knows the branch's
/// lifecycle, so the row names the run and says nothing about the checkout
/// underneath it.
#[test]
fn git_branches_lets_the_run_win_the_primary_checkout_it_adopted() {
    let (dir, repo) = init_repo();
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();
    let adopted = state.handle(req(
        "run.adopt",
        json!({ "project_id": project_id, "primary": true }),
    ));
    assert_eq!(adopted["ok"], true, "{adopted:?}");
    let run_id = run_id_of(&adopted);

    let res = state.handle(req("git.branches", json!({ "project_id": project_id })));
    assert_eq!(res["ok"], true, "{res:?}");
    let branches = res["result"]["branches"].as_array().unwrap();
    let main = branches.iter().find(|b| b["name"] == "main").unwrap();
    assert_eq!(
        main["holder"],
        json!({ "kind": "run", "id": run_id }),
        "the run that adopted the primary speaks for its branch: {main:?}"
    );
}

/// A repository whose HEAD is detached has no branch in its primary
/// checkout to hold anything. That costs the rows a primary-checkout
/// holder and nothing else — the branches are still listed, and still
/// offerable.
#[test]
fn git_branches_lists_every_branch_when_the_primary_holds_none() {
    let (dir, repo) = init_repo();
    git_in(&repo, &["branch", "feature-idle"]);
    git_in(&repo, &["checkout", "--detach"]);
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();

    let res = state.handle(req("git.branches", json!({ "project_id": project_id })));
    assert_eq!(res["ok"], true, "{res:?}");
    let branches = res["result"]["branches"].as_array().unwrap();
    assert!(
        branches.iter().any(|b| b["name"] == "feature-idle"),
        "{branches:?}"
    );
    assert!(
        branches
            .iter()
            .all(|b| b["holder"]["kind"] != json!("primary_checkout")),
        "a detached primary holds no branch: {branches:?}"
    );
}

/// A repository that cannot be read refuses the listing rather than
/// answering with rows nothing is stamped on. "Nothing holds this branch"
/// is the answer that sends the user into a checkout git refuses, so it
/// is never invented from a failed lookup.
#[test]
fn git_branches_refuses_a_project_whose_repository_cannot_be_read() {
    let dir = tempfile::tempdir().unwrap();
    let not_a_repo = dir.path().join("plain");
    std::fs::create_dir(&not_a_repo).unwrap();
    let mut state = git_gui_state(&dir, &not_a_repo);
    let project_id = state.project_at(0).id.clone();

    let res = state.handle(req("git.branches", json!({ "project_id": project_id })));

    assert_eq!(res["ok"], false, "{res:?}");
    assert!(
        res["error"]
            .as_str()
            .unwrap_or_default()
            .contains("initialize Git first"),
        "plain folders explain how to enable Git operations: {res:?}"
    );
}

/// The mirror of the refusal above, for the checkouts a row is stamped
/// from: a project whose repository vanished after registration has no
/// checkouts to ask, and the listing says so rather than answering with
/// rows nothing holds.
#[test]
fn git_branches_refuses_a_project_whose_checkouts_cannot_be_read() {
    let (dir, repo) = init_repo();
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();
    std::fs::remove_dir_all(&repo).unwrap();

    let res = state.handle(req("git.branches", json!({ "project_id": project_id })));

    assert_eq!(res["ok"], false, "{res:?}");
    assert!(
        res["result"].get("branches").is_none(),
        "no rows are invented for a repository that cannot be read: {res:?}"
    );
}

#[test]
fn git_checkout_switches_creates_and_validates() {
    let (dir, repo) = init_repo();
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();

    // Create a new branch and land on it.
    let created = state.handle(req(
        "git.checkout",
        json!({ "project_id": project_id, "branch": "feature-x", "create": true }),
    ));
    assert_eq!(created["ok"], true, "{created:?}");
    assert_eq!(created["result"]["branch"], "feature-x");

    // Switch back to an existing branch.
    let switched = state.handle(req(
        "git.checkout",
        json!({ "project_id": project_id, "branch": "main" }),
    ));
    assert_eq!(switched["result"]["branch"], "main");

    // Invalid ref names are refused before any git call.
    for bad in ["--force", "bad name", "has..dots", ""] {
        let res = state.handle(req(
            "git.checkout",
            json!({ "project_id": project_id, "branch": bad, "create": true }),
        ));
        assert_eq!(res["ok"], false, "{bad:?} -> {res:?}");
        assert!(
            res["error"]
                .as_str()
                .unwrap()
                .starts_with("invalid branch name"),
            "{res:?}"
        );
    }
}

#[test]
fn git_checkout_refuses_while_a_merge_is_in_progress() {
    let (dir, repo) = init_repo();
    // Manufacture a conflicting merge so the repo is left mid-merge.
    git_in(&repo, &["checkout", "-b", "topic"]);
    std::fs::write(repo.join("README.md"), "# topic\n").unwrap();
    git_in(&repo, &["commit", "-am", "topic"]);
    git_in(&repo, &["checkout", "main"]);
    std::fs::write(repo.join("README.md"), "# mainline\n").unwrap();
    git_in(&repo, &["commit", "-am", "mainline"]);
    git_try(&repo, &["merge", "topic"]);
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();

    let res = state.handle(req(
        "git.checkout",
        json!({ "project_id": project_id, "branch": "topic" }),
    ));
    assert_eq!(res["ok"], false, "{res:?}");
    assert_eq!(res["error"], "finish or abort the in-progress merge first");
}

#[test]
fn git_branch_delete_removes_and_force_deletes() {
    let (dir, repo) = init_repo();
    git_in(&repo, &["branch", "merged-branch"]);
    // An unmerged branch: a commit main cannot reach.
    git_in(&repo, &["checkout", "-b", "unmerged"]);
    std::fs::write(repo.join("u.txt"), "u\n").unwrap();
    git_in(&repo, &["add", "u.txt"]);
    git_in(&repo, &["commit", "-m", "unmerged work"]);
    git_in(&repo, &["checkout", "main"]);
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();

    // Deleting the current branch is git's error, passed through.
    let current = state.handle(req(
        "git.branch_delete",
        json!({ "project_id": project_id, "branch": "main" }),
    ));
    assert_eq!(current["ok"], false, "{current:?}");

    // A merged branch deletes with -d and the fresh list comes back.
    let ok = state.handle(req(
        "git.branch_delete",
        json!({ "project_id": project_id, "branch": "merged-branch" }),
    ));
    assert_eq!(ok["ok"], true, "{ok:?}");
    assert!(!ok["result"]["branches"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b["name"] == "merged-branch"));

    // An unmerged branch refuses -d, then yields to force (-D).
    let refused = state.handle(req(
        "git.branch_delete",
        json!({ "project_id": project_id, "branch": "unmerged" }),
    ));
    assert_eq!(refused["ok"], false, "{refused:?}");
    let forced = state.handle(req(
        "git.branch_delete",
        json!({ "project_id": project_id, "branch": "unmerged", "force": true }),
    ));
    assert_eq!(forced["ok"], true, "{forced:?}");
}

#[test]
fn git_stash_and_pop_round_trip() {
    let (dir, repo) = init_repo();
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();

    std::fs::write(repo.join("README.md"), "# edited\n").unwrap();
    std::fs::write(repo.join("fresh.txt"), "fresh\n").unwrap();

    // Stash includes the untracked file (-u), leaving a clean tree.
    let stashed = state.handle(req("git.stash", json!({ "project_id": project_id })));
    assert_eq!(stashed["ok"], true, "{stashed:?}");
    assert_eq!(stashed["result"]["stash_count"], 1);
    assert!(stashed["result"]["files"].as_array().unwrap().is_empty());
    assert!(!repo.join("fresh.txt").exists());

    // Pop restores both, and the stash stack is empty again.
    let popped = state.handle(req("git.stash_pop", json!({ "project_id": project_id })));
    assert_eq!(popped["ok"], true, "{popped:?}");
    assert_eq!(popped["result"]["stash_count"], 0);
    assert!(repo.join("fresh.txt").exists());

    // Popping an empty stack is git's error, passed through.
    let empty = state.handle(req("git.stash_pop", json!({ "project_id": project_id })));
    assert_eq!(empty["ok"], false, "{empty:?}");
}

#[test]
fn git_discard_reverts_tracked_and_deletes_untracked() {
    let (dir, repo) = init_repo();
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();

    // A tracked edit (staged) and a fresh untracked file.
    std::fs::write(repo.join("README.md"), "# tampered\n").unwrap();
    git_in(&repo, &["add", "README.md"]);
    std::fs::write(repo.join("junk.txt"), "junk\n").unwrap();

    let res = state.handle(req(
        "git.discard",
        json!({ "project_id": project_id, "paths": ["README.md", "junk.txt"] }),
    ));
    assert_eq!(res["ok"], true, "{res:?}");

    // Tracked file is back to its committed content, in both index and tree.
    assert_eq!(
        std::fs::read_to_string(repo.join("README.md")).unwrap(),
        "# project\n"
    );
    assert!(!has_file_entry(&res["result"], "README.md"));
    // Untracked file is gone from disk.
    assert!(!repo.join("junk.txt").exists());
    assert!(!has_file_entry(&res["result"], "junk.txt"));
}

#[test]
fn git_discard_rejects_traversal_and_symlink_escapes() {
    let (dir, repo) = init_repo();
    // A secret outside the worktree, and an untracked symlink pointing at it.
    let secret = dir.path().join("secret.txt");
    std::fs::write(&secret, "top secret\n").unwrap();
    std::os::unix::fs::symlink(&secret, repo.join("leak")).unwrap();
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();

    // Lexical traversal is refused before any git call.
    let traversal = state.handle(req(
        "git.discard",
        json!({ "project_id": project_id, "paths": ["../secret.txt"] }),
    ));
    assert_eq!(traversal["ok"], false, "{traversal:?}");

    // The symlink's components look Normal, so only the canonical fence
    // catches it — and the outside secret must survive.
    let symlink = state.handle(req(
        "git.discard",
        json!({ "project_id": project_id, "paths": ["leak"] }),
    ));
    assert_eq!(symlink["ok"], false, "{symlink:?}");
    assert!(
        secret.exists(),
        "the fence must not delete outside the worktree"
    );
}

#[test]
fn git_merge_abort_handles_each_repo_state() {
    // Clean: nothing to abort.
    let (dir, repo) = init_repo();
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();
    let clean = state.handle(req("git.merge_abort", json!({ "project_id": project_id })));
    assert_eq!(clean["ok"], false, "{clean:?}");
    assert_eq!(clean["error"], "no abortable operation in progress");

    // Merging: abort returns to a clean state.
    git_in(&repo, &["checkout", "-b", "topic"]);
    std::fs::write(repo.join("README.md"), "# topic\n").unwrap();
    git_in(&repo, &["commit", "-am", "topic"]);
    git_in(&repo, &["checkout", "main"]);
    std::fs::write(repo.join("README.md"), "# mainline\n").unwrap();
    git_in(&repo, &["commit", "-am", "mainline"]);
    git_try(&repo, &["merge", "topic"]);
    let aborted = state.handle(req("git.merge_abort", json!({ "project_id": project_id })));
    assert_eq!(aborted["ok"], true, "{aborted:?}");
    assert_eq!(aborted["result"]["repo_state"], "clean");

    // Rebasing: a conflicting rebase leaves a rebasing state to abort.
    git_in(&repo, &["checkout", "topic"]);
    git_try(&repo, &["rebase", "main"]);
    let status = state.handle(req("git.status", json!({ "project_id": project_id })));
    assert_eq!(status["result"]["repo_state"], "rebasing");
    let rebase_aborted = state.handle(req("git.merge_abort", json!({ "project_id": project_id })));
    assert_eq!(rebase_aborted["ok"], true, "{rebase_aborted:?}");
    assert_eq!(rebase_aborted["result"]["repo_state"], "clean");
}
