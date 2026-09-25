//! `workspace.reclaim` takes the workspace's local branch with it (#167),
//! under the rules Done's branch deletion keeps (#87): measured once the
//! checkout is gone, at the commit the checks passed, never a default branch,
//! never a branch checked out somewhere, never one whose default a remote
//! cannot confirm. The workspace goes either way; each linked issue says what
//! became of the branch, and why it stayed when it did.

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

fn reclaim(state: &Arc<Mutex<AppState>>, ws: &str) {
    let reclaimed = call(state, "workspace.reclaim", json!({ "workspace_id": ws }));
    assert_eq!(reclaimed["ok"], true, "{reclaimed:?}");
    assert_eq!(
        reclaimed["result"],
        json!({ "workspace_id": ws, "deleted": true }),
        "the answer is delete's, whatever became of the branch"
    );
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

    reclaim(&state, &ws);

    assert!(
        !has_branch(&source, &branch),
        "{branch} is still in {source:?}"
    );
    let entries = branch_entries(&state, &issue);
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0]["kind"], "branch_deleted");
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

    reclaim(&state, &ws);

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
