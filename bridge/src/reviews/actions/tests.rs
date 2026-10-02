use super::*;
use crate::git_fixture::{git_command, git_in, init_repo};
use crate::reviews::model::{ReviewDirectory, ReviewDirectoryStatus, ReviewSnapshot};
use crate::tracker::{Actor, Task};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

struct Fixture {
    _temp: tempfile::TempDir,
    repo: PathBuf,
    checkout: PathBuf,
    store: Store,
    request: ActionRequest,
}

fn oid(repo: &Path, name: &str) -> String {
    git2::Repository::open(repo)
        .unwrap()
        .revparse_single(name)
        .unwrap()
        .id()
        .to_string()
}

fn commit(repo: &Path, path: &str) {
    std::fs::write(repo.join(path), path).unwrap();
    git_in(repo, &["add", path]);
    git_in(repo, &["commit", "-m", path]);
}

impl Fixture {
    fn new() -> Self {
        let (temp, repo) = init_repo();
        let checkout = temp.path().join("workspace");
        git_in(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                "feature",
                checkout.to_str().unwrap(),
            ],
        );
        commit(&checkout, "feature.txt");
        let store = Store::new(temp.path().join("db")).unwrap();
        let task = store
            .create_tracker_task(
                Task::drafted(
                    repo.to_str().unwrap(),
                    "review actions",
                    Actor::User,
                    &now_rfc3339(),
                ),
                &[],
            )
            .unwrap();
        let directory = ReviewDirectory {
            id: "dir:api".into(),
            source_id: "api".into(),
            name: "API".into(),
            path: checkout.clone(),
            source_path: repo.clone(),
            is_git: true,
            status: ReviewDirectoryStatus::Git,
            reason: None,
            common_git_dir: Some(repo.join(".git")),
            branch: Some("feature".into()),
            base: None,
            head: Some(oid(&checkout, "HEAD")),
            uncommitted_files: Some(0),
        };
        store
            .save_review_snapshot(
                &task.id,
                "workspace",
                0,
                ReviewSnapshot {
                    id: "snapshot".into(),
                    number: 0,
                    created_at: now_rfc3339(),
                    author: Actor::User,
                    directories: vec![directory.clone()],
                },
            )
            .unwrap();
        let request = ActionRequest {
            params: ReviewActParams {
                task_id: task.id,
                snapshot_id: "snapshot".into(),
                expected_version: 1,
                sources: vec![SourceSelection {
                    directory_id: directory.id.clone(),
                    merge: Some(MergeSelection {
                        branch: "main".into(),
                    }),
                    push: None,
                }],
            },
            actor: Actor::User,
            sources: vec![ActionSource {
                directory,
                source_path: repo.clone(),
                error: None,
            }],
        };
        Self {
            _temp: temp,
            repo,
            checkout,
            store,
            request,
        }
    }

    fn review(&self) -> Review {
        self.store
            .load_review(&self.request.params.task_id)
            .unwrap()
            .unwrap()
    }

    fn remote(&self) -> PathBuf {
        let remote = self._temp.path().join("remote.git");
        git_in(
            self._temp.path(),
            &["init", "--bare", remote.to_str().unwrap()],
        );
        git_in(
            &self.repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        remote
    }
}

#[test]
fn destinations_cover_only_the_latest_snapshot_directories() {
    let fixture = Fixture::new();
    let mut review = fixture.review();
    let mut latest = review.snapshots[0].clone();
    latest.id = "latest".into();
    latest.number += 1;
    latest.directories[0].id = "new-directory".into();
    review.snapshots.push(latest.clone());
    let mut sources = fixture.request.sources.clone();
    sources.push(ActionSource {
        directory: latest.directories[0].clone(),
        source_path: fixture.repo.clone(),
        error: None,
    });

    let result = with_destinations(review, &sources);

    assert_eq!(result.snapshots.len(), 2, "snapshot history stays available");
    assert_eq!(result.destinations.len(), 1);
    let destination = &result.destinations[0];
    assert_eq!(destination.snapshot_id, "latest");
    assert_eq!(destination.directory_id, "new-directory");
    assert!(destination.branches.contains(&"main".to_string()));
    assert!(destination.error.is_none());
}

#[test]
fn merge_is_recorded_before_push_and_retry_push_uses_its_tip() {
    let mut fixture = Fixture::new();
    fixture.request.params.sources[0].push = Some(PushSelection {
        remote: "origin".into(),
        branch: "published".into(),
        merge_action_id: None,
    });
    // Dirty source content is excluded, and remains exactly as it was.
    std::fs::write(fixture.checkout.join("uncommitted"), "keep me").unwrap();
    let review = act(&fixture.store, &fixture.request, || {}).unwrap();
    let row = &review.actions[0];
    assert_eq!(row.status, ActionStatus::Failed);
    assert_eq!(row.steps[0].status, StepStatus::Succeeded);
    assert_eq!(row.steps[1].status, StepStatus::Failed);
    let tip = row.steps[0].result_head.clone().unwrap();
    assert_eq!(row.steps[1].input_head.as_deref(), Some(tip.as_str()));
    assert_eq!(oid(&fixture.repo, "main"), tip);
    let remote = fixture.remote();
    fixture.request.params.expected_version = review.version;
    fixture.request.params.sources[0].merge = None;
    fixture.request.params.sources[0]
        .push
        .as_mut()
        .unwrap()
        .merge_action_id = Some(row.id.clone());
    let done = act(&fixture.store, &fixture.request, || {}).unwrap();
    assert_eq!(done.actions[1].steps.len(), 1);
    assert_eq!(done.actions[1].status, ActionStatus::Succeeded);
    assert_eq!(oid(&remote, "published"), tip);
    assert_eq!(
        oid(&fixture.repo, "main"),
        tip,
        "push retry never reruns merge"
    );
    assert_eq!(
        std::fs::read_to_string(fixture.checkout.join("uncommitted")).unwrap(),
        "keep me"
    );
    assert_eq!(
        fixture
            .store
            .load_tracker_task(&fixture.request.params.task_id)
            .unwrap()
            .unwrap()
            .status,
        "backlog"
    );
    assert!(fixture.checkout.exists());
}

#[test]
fn failed_source_keeps_success_from_another_source_and_skips_later_step() {
    let mut fixture = Fixture::new();
    let mut second = fixture.request.sources[0].clone();
    second.directory.id = "dir:web".into();
    second.directory.source_id = "web".into();
    second.directory.name = "Web".into();
    second.error = Some("Source unavailable".into());
    let mut snapshot = fixture.review().snapshots.remove(0);
    snapshot.id = "both".into();
    snapshot.directories.push(second.directory.clone());
    let (review, _) = fixture
        .store
        .save_review_snapshot(&fixture.request.params.task_id, "workspace", 1, snapshot)
        .unwrap();
    fixture.request.params.snapshot_id = "both".into();
    fixture.request.params.expected_version = review.version;
    fixture.request.params.sources.push(SourceSelection {
        directory_id: second.directory.id.clone(),
        merge: Some(MergeSelection {
            branch: "main".into(),
        }),
        push: Some(PushSelection {
            remote: "origin".into(),
            branch: "main".into(),
            merge_action_id: None,
        }),
    });
    fixture.request.sources.push(second);
    let review = act(&fixture.store, &fixture.request, || {}).unwrap();
    assert_eq!(review.actions[0].status, ActionStatus::Succeeded);
    assert_eq!(review.actions[1].status, ActionStatus::Failed);
    assert_eq!(review.actions[1].steps[1].status, StepStatus::Pending);
    assert!(fixture.repo.join("feature.txt").exists());
}

#[test]
fn stale_click_runs_no_git_and_source_actions_have_no_review_state_gate() {
    let mut fixture = Fixture::new();
    let old = oid(&fixture.repo, "main");
    fixture.request.params.expected_version = 0;
    assert!(act(&fixture.store, &fixture.request, || {})
        .unwrap_err()
        .starts_with("stale_version:"));
    assert_eq!(oid(&fixture.repo, "main"), old);
    assert!(fixture.review().actions.is_empty());
    let review = fixture
        .store
        .complete_review(
            &fixture.request.params.task_id,
            1,
            &Actor::User,
            "Handled externally",
        )
        .unwrap();
    fixture.request.params.expected_version = review.version;
    let review = act(&fixture.store, &fixture.request, || {}).unwrap();
    assert_eq!(review.actions[0].status, ActionStatus::Succeeded);
    assert_eq!(review.completion.unwrap().description, "Handled externally");
}

#[test]
fn leave_unchanged_does_not_write_a_result_or_advance_the_review() {
    let mut fixture = Fixture::new();
    fixture.request.params.sources[0].merge = None;
    let before = oid(&fixture.repo, "HEAD");
    let review = act(&fixture.store, &fixture.request, || {}).unwrap();
    assert_eq!(review.version, 1);
    assert!(review.actions.is_empty());
    assert_eq!(oid(&fixture.repo, "HEAD"), before);
}

#[test]
fn a_new_merge_replaces_the_push_retries_old_merge_provenance() {
    let mut fixture = Fixture::new();
    let first = act(&fixture.store, &fixture.request, || {}).unwrap();
    commit(&fixture.repo, "later-base.txt");
    fixture.request.params.expected_version = first.version;
    fixture.request.params.sources[0].push = Some(PushSelection {
        remote: "origin".into(),
        branch: "published".into(),
        merge_action_id: Some(first.actions[0].id.clone()),
    });
    let review = act(&fixture.store, &fixture.request, || {}).unwrap();
    let fresh = &review.actions[1];
    assert_eq!(fresh.steps[0].status, StepStatus::Succeeded);
    assert_eq!(fresh.steps[1].input_head, fresh.steps[0].result_head);
    assert_ne!(
        fresh.steps[1].input_head,
        first.actions[0].steps[0].result_head
    );
    assert_eq!(
        fresh.steps[1].merge_action_id, None,
        "the current source row now owns the merge tip"
    );
}

#[test]
fn failed_progress_write_stops_git_and_releases_the_running_source() {
    let fixture = Fixture::new();
    let before = oid(&fixture.repo, "main");
    let notifications = std::cell::Cell::new(0);
    let error = act(&fixture.store, &fixture.request, || {
        notifications.set(notifications.get() + 1);
        if notifications.get() == 1 {
            fixture.store.fail_next_write();
        }
    })
    .unwrap_err();
    assert!(error.contains("injected store failure"));
    assert_eq!(oid(&fixture.repo, "main"), before);
    assert_eq!(
        fixture.review().actions[0].status,
        ActionStatus::Interrupted
    );
}

#[test]
fn failure_to_save_merge_result_preserves_tip_and_does_not_start_push() {
    let mut fixture = Fixture::new();
    let remote = fixture.remote();
    fixture.request.params.sources[0].push = Some(PushSelection {
        remote: "origin".into(),
        branch: "published".into(),
        merge_action_id: None,
    });
    let notifications = std::cell::Cell::new(0);
    assert!(act(&fixture.store, &fixture.request, || {
        notifications.set(notifications.get() + 1);
        if notifications.get() == 2 {
            fixture.store.fail_next_write();
        }
    })
    .is_err());
    let review = fixture.review();
    let row = &review.actions[0];
    assert_eq!(row.status, ActionStatus::Interrupted);
    assert_eq!(row.steps[0].status, StepStatus::Succeeded);
    assert_eq!(
        row.steps[0].result_head.as_deref(),
        Some(oid(&fixture.repo, "main").as_str())
    );
    assert_eq!(row.steps[1].status, StepStatus::Interrupted);
    assert!(git2::Repository::open_bare(remote)
        .unwrap()
        .find_reference("refs/heads/published")
        .is_err());
}

fn contended_merge(unchecked: bool, acquire_checkout: bool) {
    let fixture = Fixture::new();
    let branch = if unchecked { "release" } else { "main" };
    if unchecked {
        git_in(&fixture.repo, &["branch", branch, "main"]);
    }
    let remote = fixture.remote();
    git_in(
        &fixture.repo,
        &["push", "origin", &format!("main:refs/heads/{branch}")],
    );
    let writer = fixture._temp.path().join("remote-writer");
    git_in(
        fixture._temp.path(),
        &[
            "clone",
            "--branch",
            branch,
            remote.to_str().unwrap(),
            writer.to_str().unwrap(),
        ],
    );
    commit(&writer, "synced.txt");
    git_in(&writer, &["push", "origin", branch]);
    let mut request = fixture.request;
    request.params.sources[0].merge.as_mut().unwrap().branch = branch.into();
    let (held, _) = SyncLock::acquire(&fixture.repo, Duration::from_secs(1)).unwrap();
    let (accepted_tx, accepted_rx) = mpsc::channel();
    std::thread::scope(|scope| {
        let worker = scope.spawn(|| {
            act(&fixture.store, &request, || {
                let _ = accepted_tx.send(());
            })
        });
        accepted_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(!fixture.repo.join("feature.txt").exists());
        // Run the same sync under the same configured-path lock as the daemon.
        // It moves the target before the waiting review re-reads placement.
        let sync = crate::source_sync::sync_base(
            &fixture.repo,
            branch,
            crate::source_sync::Fetch::Within(Duration::from_secs(5)),
        );
        assert!(
            matches!(
                sync.outcome,
                crate::source_sync::SyncOutcome::FastForwarded { .. }
            ),
            "{sync:?}"
        );
        let elsewhere = fixture._temp.path().join("target");
        if acquire_checkout {
            git_in(
                &fixture.repo,
                &["worktree", "add", elsewhere.to_str().unwrap(), branch],
            );
        }
        drop(held);
        let review = worker.join().unwrap().unwrap();
        assert_eq!(
            review.actions[0].status,
            ActionStatus::Succeeded,
            "{:?}",
            review.actions
        );
        let tree = git_command(&fixture.repo, &["ls-tree", "--name-only", branch])
            .output()
            .unwrap();
        let files = String::from_utf8(tree.stdout).unwrap();
        assert!(files.contains("synced.txt") && files.contains("feature.txt"));
        if acquire_checkout {
            assert!(elsewhere.join("feature.txt").exists());
            assert!(elsewhere.join("synced.txt").exists());
        }
    });
}

#[test]
fn sync_lock_serializes_checked_out_merge_and_rereads_tip() {
    contended_merge(false, false);
}

#[test]
fn sync_lock_serializes_temporary_merge_and_rereads_tip() {
    contended_merge(true, false);
}

#[test]
fn target_acquired_while_waiting_is_merged_in_its_new_checkout() {
    contended_merge(true, true);
}
