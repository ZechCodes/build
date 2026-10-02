use super::*;
use crate::git_fixture::init_repo;
use crate::tracker::Task;
use crate::workspace::{DirectoryStatus, WorkspaceDirectory, WorkspaceStatus};
use std::path::Path;
use std::sync::Barrier;

fn request(store: &Store, repo: &Path) -> SnapshotRequest {
    let task = store
        .create_tracker_task(
            Task::drafted(
                repo.to_str().unwrap(),
                "Snapshot race",
                Actor::User,
                &crate::store::now_rfc3339(),
            ),
            &[],
        )
        .unwrap();
    SnapshotRequest {
        task_id: task.id,
        expected_version: 0,
        base_overrides: BTreeMap::new(),
        author: Actor::User,
        workspace: Workspace {
            id: "workspace-1".into(),
            project_id: "project-1".into(),
            name: "Review".into(),
            root: repo.into(),
            status: WorkspaceStatus::Ready,
            archived_at: None,
            isolation: Default::default(),
            managed: false,
            created_by_agent: false,
            directories: vec![WorkspaceDirectory {
                id: "dir-1".into(),
                source_id: "source-1".into(),
                name: "Source".into(),
                path: repo.into(),
                source_path: repo.into(),
                is_git: true,
                branch: Some("main".into()),
                base_branch: "main".into(),
                status: DirectoryStatus::Ready,
                effective_isolation: None,
                finished_head: None,
                error: None,
            }],
        },
    }
}

fn pins(repo: &Path) -> Vec<String> {
    git2::Repository::open(repo)
        .unwrap()
        .references_glob("refs/build/reviews/*")
        .unwrap()
        .map(|reference| reference.unwrap().name().unwrap().to_string())
        .collect()
}

#[test]
fn simultaneous_snapshots_leave_only_the_winners_refs() {
    let (_home, repo) = init_repo();
    let db = tempfile::tempdir().unwrap();
    let store = Store::new(db.path()).unwrap();
    let request = request(&store, &repo);
    let start = Barrier::new(2);
    let results = std::thread::scope(|scope| {
        let first = scope.spawn(|| {
            start.wait();
            snapshot(&store, &request)
        });
        let second = scope.spawn(|| {
            start.wait();
            snapshot(&store, &request)
        });
        [first.join().unwrap(), second.join().unwrap()]
    });
    assert_eq!(
        results.iter().filter(|result| result.is_ok()).count(),
        1,
        "{results:?}"
    );
    let loser = results
        .iter()
        .find_map(|result| result.as_ref().err())
        .unwrap();
    assert!(loser.starts_with("stale_version:"), "{loser}");
    let winner = store.load_review(&request.task_id).unwrap().unwrap();
    assert_eq!(winner.version, 1);
    assert_eq!(winner.snapshots.len(), 1);
    let refs = pins(&repo);
    assert_eq!(refs.len(), 2, "{refs:?}");
    assert!(refs
        .iter()
        .all(|name| name.contains(&winner.snapshots[0].id)));
}

#[test]
fn completion_keeps_pins_and_explicit_history_deletion_releases_them() {
    let (_home, repo) = init_repo();
    let db = tempfile::tempdir().unwrap();
    let store = Store::new(db.path()).unwrap();
    let request = request(&store, &repo);
    let saved = snapshot(&store, &request).unwrap();
    let before = pins(&repo);
    assert_eq!(before.len(), 2);
    store
        .complete_review(
            &request.task_id,
            saved.version,
            &Actor::User,
            "Reviewed and pushed",
        )
        .unwrap();
    assert_eq!(pins(&repo), before);
    delete_project_history(&store, repo.to_str().unwrap()).unwrap();
    assert!(pins(&repo).is_empty());
    assert!(store.load_review(&request.task_id).unwrap().is_none());
    assert!(store.load_tracker_task(&request.task_id).unwrap().is_none());
}

#[test]
fn a_failed_ref_release_keeps_history_for_an_explicit_retry() {
    let (_home, repo) = init_repo();
    let db = tempfile::tempdir().unwrap();
    let store = Store::new(db.path()).unwrap();
    let request = request(&store, &repo);
    let saved = snapshot(&store, &request).unwrap();
    let repository = git2::Repository::open(&repo).unwrap();
    let name = pins(&repo)
        .into_iter()
        .find(|name| name.ends_with("/head"))
        .unwrap();
    let expected = repository.refname_to_id(&name).unwrap();
    let changed = repository.blob(b"some other object").unwrap();
    repository
        .reference(&name, changed, true, "external ref movement")
        .unwrap();
    assert!(delete_project_history(&store, repo.to_str().unwrap()).is_err());
    assert_eq!(store.load_review(&request.task_id).unwrap().unwrap(), saved);
    assert_eq!(repository.refname_to_id(&name).unwrap(), changed);
    repository
        .reference(&name, expected, true, "restore fixture pin")
        .unwrap();
    delete_project_history(&store, repo.to_str().unwrap()).unwrap();
    assert!(pins(&repo).is_empty());
}

#[test]
fn another_workspace_replaces_the_review_and_releases_old_pins() {
    let (_home, repo) = init_repo();
    let (_new_home, new_repo) = init_repo();
    let db = tempfile::tempdir().unwrap();
    let store = Store::new(db.path()).unwrap();
    let mut request = request(&store, &repo);
    let first = snapshot(&store, &request).unwrap();
    let task_before = store.load_tracker_task(&request.task_id).unwrap().unwrap();
    request.expected_version = first.version;
    request.workspace.id = "workspace-2".into();
    request.workspace.root = new_repo.clone();
    request.workspace.directories[0].path = new_repo.clone();
    let replacement = snapshot(&store, &request).unwrap();
    assert_eq!(replacement.workspace_id, "workspace-2");
    assert_eq!(replacement.version, first.version + 1);
    assert_eq!(replacement.snapshots.len(), 1);
    assert_eq!(replacement.snapshots[0].number, 2);
    assert_ne!(replacement.snapshots[0].id, first.snapshots[0].id);
    assert!(pins(&repo).is_empty());
    assert_eq!(pins(&new_repo).len(), 2);
    assert!(pins(&new_repo)
        .iter()
        .all(|name| name.contains(&replacement.snapshots[0].id)));
    assert_eq!(
        store.load_tracker_task(&request.task_id).unwrap().unwrap(),
        task_before
    );
    drop(store);
    let reopened = Store::new(db.path()).unwrap();
    assert_eq!(
        reopened.load_review(&request.task_id).unwrap().unwrap(),
        replacement
    );
}

#[test]
fn rejected_workspace_replacement_keeps_the_old_review_and_pins() {
    let (_home, repo) = init_repo();
    let (_new_home, new_repo) = init_repo();
    let db = tempfile::tempdir().unwrap();
    let store = Store::new(db.path()).unwrap();
    let mut request = request(&store, &repo);
    let before = snapshot(&store, &request).unwrap();
    let before_pins = pins(&repo);
    request.workspace.id = "workspace-2".into();
    request.workspace.directories[0].path = new_repo.clone();
    let stale = snapshot(&store, &request).unwrap_err();
    assert!(stale.starts_with("stale_version:"));
    assert_eq!(
        store.load_review(&request.task_id).unwrap().unwrap(),
        before
    );
    assert_eq!(pins(&repo), before_pins);
    assert!(pins(&new_repo).is_empty());
    request.expected_version = before.version;
    store.fail_next_write();
    assert!(snapshot(&store, &request).is_err());
    assert_eq!(
        store.load_review(&request.task_id).unwrap().unwrap(),
        before
    );
    assert_eq!(pins(&repo), before_pins);
    assert!(pins(&new_repo).is_empty());
}
