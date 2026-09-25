//! `workspace.reclaim` takes the workspace's local branch with it (#167),
//! under the rules Done's branch deletion keeps (#87): measured once the
//! checkout is gone, at the commit the checks passed, never a default branch,
//! never a branch checked out somewhere, never one whose default a remote
//! cannot confirm. The workspace goes either way. The answer says what became
//! of each branch in each repository, and so does each linked issue, with why
//! a branch stayed when it did.

use super::workspace_reclaim::{call, finish, linked_workspace, root_and_checkout};
use super::*;
use crate::app::{DeferredJob, DeferredNext};
use crate::git_fixture::git_in;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// The source repository the workspace's one checkout was cut from, and the
/// branch that checkout carries.
fn source_and_branch(state: &Arc<Mutex<AppState>>, ws: &str) -> (PathBuf, String) {
    let app = state.lock().unwrap();
    let directory = &app.workspaces.get(ws).unwrap().directories[0];
    (
        directory.source_path.clone(),
        directory.branch.clone().expect("a branch"),
    )
}

fn has_branch(repo: &Path, branch: &str) -> bool {
    std::process::Command::new("git")
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .current_dir(repo)
        .status()
        .unwrap()
        .success()
}

/// The `branch_*` entries on the issue's timeline.
fn branch_entries(state: &Arc<Mutex<AppState>>, issue: &str) -> Vec<Value> {
    call(state, "issues.get", json!({ "issue_id": issue }))["result"]["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| {
            entry["kind"]
                .as_str()
                .is_some_and(|kind| kind.starts_with("branch_"))
        })
        .cloned()
        .collect()
}

/// Reclaim, and answer what the reclaim said of its branches.
fn reclaim(state: &Arc<Mutex<AppState>>, ws: &str) -> Vec<Value> {
    let reclaimed = call(state, "workspace.reclaim", json!({ "workspace_id": ws }));
    assert_eq!(reclaimed["ok"], true, "{reclaimed:?}");
    assert_eq!(reclaimed["result"]["workspace_id"], ws);
    assert_eq!(
        reclaimed["result"]["deleted"], true,
        "the workspace goes whatever became of the branch"
    );
    reclaimed["result"]["branches"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("no branches in {reclaimed:?}"))
}

/// The id of the workspace's first source, as the answer names it.
fn source_id(state: &Arc<Mutex<AppState>>, ws: &str) -> String {
    state
        .lock()
        .unwrap()
        .workspaces
        .get(ws)
        .unwrap()
        .directories[0]
        .source_id
        .clone()
}

/// The origin the fixture's source repository pushes to.
fn origin_of(source: &Path) -> PathBuf {
    PathBuf::from(
        crate::git_process::run_git(source, &["remote", "get-url", "origin"])
            .unwrap()
            .trim(),
    )
}

#[test]
fn reclaim_deletes_the_workspace_branch_and_says_so_on_the_issue() {
    let (_tmp, state, _project, ws, issue) = linked_workspace();
    finish(&state, &issue);
    let (source, branch) = source_and_branch(&state, &ws);
    assert!(has_branch(&source, &branch));
    let source_id = source_id(&state, &ws);

    let branches = reclaim(&state, &ws);

    assert!(
        !has_branch(&source, &branch),
        "{branch} is still in {source:?}"
    );
    assert_eq!(
        branches,
        vec![json!({
            "source_id": source_id,
            "repository": source.display().to_string(),
            "branch": branch,
            "outcome": "deleted",
        })]
    );
    let entries = branch_entries(&state, &issue);
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0]["kind"], "branch_deleted");
    assert_eq!(entries[0]["payload"]["outcome"], "deleted");
    assert_eq!(
        entries[0]["payload"]["repository"],
        source.display().to_string()
    );
    assert_eq!(entries[0]["actor"], json!({ "kind": "user" }));
    assert_eq!(entries[0]["payload"]["branch"], branch.as_str());
    assert_eq!(entries[0]["payload"]["workspace_id"], ws.as_str());
    assert_eq!(entries[0]["payload"]["reclaimed"], true);
    assert!(entries[0]["payload"].get("reason").is_none(), "{entries:?}");
}

/// The source's own checkout stood on the branch: the workspace goes, the
/// branch stays, and the issue says where it is checked out.
#[test]
fn reclaim_keeps_a_branch_checked_out_in_the_source_and_says_why() {
    let (_tmp, state, _project, ws, issue) = linked_workspace();
    finish(&state, &issue);
    let (root, _) = root_and_checkout(&state, &ws);
    let (source, branch) = source_and_branch(&state, &ws);
    // Same commit as main, so the source's tree stays clean.
    git_in(
        &source,
        &["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")],
    );

    let branches = reclaim(&state, &ws);

    assert_eq!(branches.len(), 1, "{branches:?}");
    assert_eq!(branches[0]["outcome"], "kept");
    assert!(
        branches[0]["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("it is checked out at ")),
        "{branches:?}"
    );
    assert!(!root.exists(), "the workspace goes either way");
    assert!(has_branch(&source, &branch));
    let entries = branch_entries(&state, &issue);
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0]["kind"], "branch_kept");
    assert_eq!(entries[0]["payload"]["branch"], branch.as_str());
    let reason = entries[0]["payload"]["reason"].as_str().unwrap();
    assert!(
        reason.starts_with(&format!(
            "Build cannot delete the branch {branch}: it is checked out at "
        )),
        "{reason}"
    );
}

/// The remote calls the workspace's branch its default: it stays.
#[test]
fn reclaim_keeps_the_branch_a_remote_calls_its_default() {
    let (_tmp, state, _project, ws, issue) = linked_workspace();
    finish(&state, &issue);
    let (source, branch) = source_and_branch(&state, &ws);
    let origin = origin_of(&source);
    git_in(&source, &["push", "-q", "origin", &branch]);
    git_in(
        &origin,
        &["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")],
    );

    reclaim(&state, &ws);

    assert!(has_branch(&source, &branch));
    let entries = branch_entries(&state, &issue);
    assert_eq!(entries[0]["kind"], "branch_kept", "{entries:?}");
    assert_eq!(
        entries[0]["payload"]["reason"],
        format!("Build cannot delete the branch {branch}: it is a default branch.")
    );
}

/// A remote that cannot be asked which branch its `HEAD` names leaves the
/// branch: nothing can say it is not that remote's default.
#[test]
fn reclaim_keeps_the_branch_when_the_remote_default_cannot_be_confirmed() {
    let (tmp, state, _project, ws, issue) = linked_workspace();
    finish(&state, &issue);
    let (source, branch) = source_and_branch(&state, &ws);
    let gone = tmp.path().join("no-such-origin.git");
    git_in(
        &source,
        &["remote", "set-url", "origin", gone.to_str().unwrap()],
    );

    reclaim(&state, &ws);

    assert!(has_branch(&source, &branch));
    let entries = branch_entries(&state, &issue);
    assert_eq!(entries[0]["kind"], "branch_kept", "{entries:?}");
    assert_eq!(
        entries[0]["payload"]["reason"],
        format!(
            "Build cannot delete the branch {branch}: Build could not confirm it is not the remote's default branch."
        )
    );
}

/// The measurement and the decision under the lock, and the removal
/// handed on: `(params, the removal still to run)`.
fn decided(state: &Arc<Mutex<AppState>>, ws: &str) -> (Value, DeferredJob) {
    let params = json!({ "workspace_id": ws });
    let (answered, deferred) = state
        .lock()
        .unwrap()
        .dispatch_deferring("workspace.reclaim", &params);
    assert!(answered.is_ok(), "{answered:?}");
    let measured = deferred.expect("the Git measurement leaves the lock").run();
    let next = state
        .lock()
        .unwrap()
        .apply_deferred_stage("workspace.reclaim", &params, measured);
    match next {
        DeferredNext::Again(removing) => (params, removing),
        DeferredNext::Answered(answer) => panic!("the removal was not handed on: {answer:?}"),
    }
}

fn settle(state: &Arc<Mutex<AppState>>, params: &Value, removing: DeferredJob) {
    let removed = removing.run();
    let next = state
        .lock()
        .unwrap()
        .apply_deferred_stage("workspace.reclaim", params, removed);
    let DeferredNext::Answered(answer) = next else {
        panic!("a third stage");
    };
    assert!(answer.is_ok(), "{answer:?}");
}

/// Nothing is measured or deleted under the lock: the decision leaves the
/// branch where it is, and the removal, off the lock, takes it once the
/// checkout is gone.
#[test]
fn the_branch_goes_in_the_removal_off_the_lock() {
    let (_tmp, state, _project, ws, issue) = linked_workspace();
    finish(&state, &issue);
    let (source, branch) = source_and_branch(&state, &ws);

    let (params, removing) = decided(&state, &ws);
    assert!(has_branch(&source, &branch), "decided under the lock only");
    settle(&state, &params, removing);

    assert!(!has_branch(&source, &branch));
}

/// A commit no remote has lands on the branch after reclaim measured and
/// decided: the removal still goes ahead, and the branch keeps the commit.
#[test]
fn a_commit_landing_after_the_decision_keeps_the_branch() {
    let (_tmp, state, _project, ws, issue) = linked_workspace();
    finish(&state, &issue);
    let (source, branch) = source_and_branch(&state, &ws);

    let (params, removing) = decided(&state, &ws);
    let tip = crate::git_process::run_git(&source, &["rev-parse", &branch])
        .unwrap()
        .trim()
        .to_string();
    let local_only = crate::git_process::run_git(
        &source,
        &[
            "commit-tree",
            &format!("{tip}^{{tree}}"),
            "-p",
            &tip,
            "-m",
            "only here",
        ],
    )
    .unwrap()
    .trim()
    .to_string();
    git_in(
        &source,
        &[
            "update-ref",
            &format!("refs/heads/{branch}"),
            &local_only,
            &tip,
        ],
    );
    settle(&state, &params, removing);

    assert!(has_branch(&source, &branch));
    let entries = branch_entries(&state, &issue);
    assert_eq!(entries[0]["kind"], "branch_kept", "{entries:?}");
    assert_eq!(
        entries[0]["payload"]["reason"],
        format!("Build cannot delete the branch {branch}: it has commits no remote has.")
    );
}

/// A branch somebody deleted after the decision: nothing to take, and nothing
/// said about it on the issue.
#[test]
fn a_branch_already_gone_is_not_reported() {
    let (_tmp, state, _project, ws, issue) = linked_workspace();
    finish(&state, &issue);
    let (source, branch) = source_and_branch(&state, &ws);

    let (params, removing) = decided(&state, &ws);
    git_in(
        &source,
        &["update-ref", "-d", &format!("refs/heads/{branch}")],
    );
    settle(&state, &params, removing);

    assert!(!has_branch(&source, &branch));
    assert_eq!(branch_entries(&state, &issue), Vec::<Value>::new());
}

/// Two sources carrying the same branch name, deleted in one repository and
/// kept in the other: the answer and the issue say which is which.
#[test]
fn a_branch_deleted_in_one_source_and_kept_in_another_is_told_apart() {
    let (tmp, state, project, issue) = two_source_project();
    let ws = super::project_agent::workspace(&mut state.lock().unwrap(), &project, "pair");
    let linked = call(
        &state,
        "issues.link",
        json!({ "issue_id": issue, "workspace_id": ws }),
    );
    assert_eq!(linked["ok"], true, "{linked:?}");
    finish(&state, &issue);
    let directories = state
        .lock()
        .unwrap()
        .workspaces
        .get(&ws)
        .unwrap()
        .directories
        .clone();
    assert_eq!(directories.len(), 2, "{directories:?}");
    let branch = directories[0].branch.clone().unwrap();
    assert_eq!(directories[1].branch.as_deref(), Some(branch.as_str()));
    let (kept, deleted) = (&directories[1], &directories[0]);
    git_in(
        &kept.source_path,
        &["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")],
    );

    let mut branches = reclaim(&state, &ws);

    branches.sort_by_key(|entry| entry["outcome"].as_str().unwrap().to_string());
    assert_eq!(branches.len(), 2, "{branches:?}");
    assert_eq!(branches[0]["outcome"], "deleted");
    assert_eq!(branches[0]["source_id"], deleted.source_id.as_str());
    assert_eq!(
        branches[0]["repository"],
        deleted.source_path.display().to_string()
    );
    assert_eq!(branches[1]["outcome"], "kept");
    assert_eq!(branches[1]["source_id"], kept.source_id.as_str());
    assert!(!has_branch(&deleted.source_path, &branch));
    assert!(has_branch(&kept.source_path, &branch));
    let entries = branch_entries(&state, &issue);
    let by_repository = |repo: &Path| {
        entries
            .iter()
            .find(|entry| entry["payload"]["repository"] == repo.display().to_string())
            .unwrap_or_else(|| panic!("no entry for {repo:?} in {entries:?}"))
    };
    assert_eq!(
        by_repository(&deleted.source_path)["kind"],
        "branch_deleted"
    );
    assert_eq!(by_repository(&kept.source_path)["kind"], "branch_kept");
    drop(tmp);
}

/// A project over two repositories with origins, and an issue:
/// `(tempdir, state, project, issue)`.
fn two_source_project() -> (tempfile::TempDir, Arc<Mutex<AppState>>, String, String) {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (mut state, project_id) = super::tracker::tracked_with_origin(&state_root);
    let second = crate::git_fixture::init_repo_named(&state_root, "second");
    let origin = state_root.join("second.git");
    git_in(
        &state_root,
        &[
            "clone",
            "--bare",
            second.to_str().unwrap(),
            origin.to_str().unwrap(),
        ],
    );
    git_in(
        &second,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git_in(&second, &["fetch", "origin"]);
    let added = state.handle(req(
        "project.add_source",
        json!({ "project_id": project_id, "path": second, "name": "second" }),
    ));
    assert_eq!(added["ok"], true, "{added:?}");
    let issue = super::tracker::filed(&mut state, &project_id, "Two sources")["id"]
        .as_str()
        .unwrap()
        .to_string();
    (tmp, state.shared(), project_id, issue)
}
