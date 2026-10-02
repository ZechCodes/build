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
