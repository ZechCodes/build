//! Retrying a persisted workspace uses its saved sources, not today's project.

use super::*;
use crate::app::tests::workflow::lifecycle_offlock::frame_on_a_thread;
use crate::source_sync::SyncLock;
use crate::workspace::{DirectoryStatus, WorkspaceRegistry, WorkspaceSource, WorkspaceStatus};
use std::time::Duration;

const WAITING: Duration = Duration::from_millis(400);
const ANSWERED: Duration = Duration::from_secs(30);

struct RetryFixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    state: AppState,
    project_id: String,
    workspace_id: String,
    workspace_root: PathBuf,
    saved_repo: PathBuf,
    ready_source: PathBuf,
    ready_file: PathBuf,
    pending_path: PathBuf,
}

fn fixture(ready_git: bool) -> RetryFixture {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let ready = if ready_git {
        init_repo_named(&root, "ready-source")
    } else {
        let path = root.join("ready-source");
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("kept.txt"), "source contents").unwrap();
        path
    };
    let saved_repo = init_repo_named(&root, "pending-source");
    let config = root.join("config.json");
    std::fs::write(&config, "{}").unwrap();
    let mut state = app(&root).with_config(&config).unwrap();
    let opened = state.handle(req(
        "project.create",
        json!({
            "name": "retry-project",
            "sources": [{"name": "ready", "path": ready}, {"name": "pending", "path": saved_repo}]
        }),
    ));
    assert_eq!(opened["ok"], true, "{opened}");
    let project_id = opened["result"]["project_id"].as_str().unwrap().to_string();
    let sources: Vec<_> = state
        .projects
        .get(&project_id)
        .unwrap()
        .sources
        .iter()
        .map(|source| WorkspaceSource {
            id: source.id.clone(),
            name: source.name.clone(),
            mount: source.mount.clone(),
            path: source.path.clone(),
            is_git: source.is_git,
            base_branch: source.base_branch.clone(),
        })
        .collect();
    let workspace = state
        .workspaces
        .begin_with_isolation(&project_id, "retry", &sources, Isolation::Worktree)
        .unwrap();
    let rift_root = root.join("unused-rift");
    let failed = state.workspaces.provision(
        &workspace.id,
        &sources,
        Isolation::Worktree,
        |source, path, isolation| {
            if source.id == "source-2" {
                return Err("fixture provisioning failure".into());
            }
            crate::app::workspaces::materialize_source(
                source, path, "retry", isolation, &rift_root, None,
            )
        },
    );
    assert!(failed.is_err());
    // Persisted failure and a completed directory survive a registry restart.
    state.workspaces = WorkspaceRegistry::recover(state.workspaces.root()).unwrap();
    let saved = state.workspaces.get(&workspace.id).unwrap();
    assert_eq!(saved.status, WorkspaceStatus::Failed);
    assert_eq!(saved.directories[0].status, DirectoryStatus::Ready);
    let ready_file = saved.directories[0].path.join("user.txt");
    let pending_path = saved.directories[1].path.clone();
    std::fs::write(&ready_file, "keep completed work").unwrap();
    RetryFixture {
        _dir: dir,
        root,
        state,
        project_id,
        workspace_id: workspace.id,
        workspace_root: workspace.root,
        saved_repo,
        ready_source: ready,
        ready_file,
        pending_path,
    }
}

fn kept(fixture: &RetryFixture) {
    assert!(fixture
        .workspace_root
        .join(crate::workspace::MANIFEST_FILE)
        .is_file());
    assert_eq!(
        std::fs::read_to_string(&fixture.ready_file).unwrap(),
        "keep completed work"
    );
}

fn oid(path: &Path, name: &str) -> String {
    git2::Repository::open(path)
        .unwrap()
        .revparse_single(name)
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string()
}

fn retry(fixture: &mut RetryFixture) -> Value {
    fixture.state.handle(req(
        "workspace.retry",
        json!({"workspace_id": fixture.workspace_id}),
    ))
}

#[test]
fn retry_ignores_an_unrelated_unborn_source_with_sync_off() {
    let mut fixture = fixture(false);
    let unborn = fixture.root.join("unborn");
    std::fs::create_dir(&unborn).unwrap();
    git_in(&unborn, &["init", "-q", "-b", "main"]);
    let added = fixture.state.handle(req(
        "project.add_source",
        json!({"project_id": fixture.project_id, "path": unborn, "name": "unborn"}),
    ));
    assert_eq!(added["ok"], true, "{added}");
    let disabled = fixture.state.handle(req(
        "project.update_source",
        json!({"project_id": fixture.project_id, "source_id": "source-3", "sync_base": false}),
    ));
    assert_eq!(disabled["ok"], true, "{disabled}");
    let reply = retry(&mut fixture);
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(reply["result"]["directories"].as_array().unwrap().len(), 2);
    assert_eq!(
        oid(&fixture.pending_path, "HEAD"),
        oid(&fixture.saved_repo, "main")
    );
    assert!(!fixture.workspace_root.join("unborn").exists());
    kept(&fixture);
}

enum Drift {
    Move,
    Remove,
    Base,
}

fn drift(fixture: &mut RetryFixture, change: Drift) {
    let reply = match change {
        Drift::Remove => fixture.state.handle(req(
            "project.remove_source",
            json!({"project_id": fixture.project_id, "source_id": "source-2"}),
        )),
        Drift::Move => {
            let replacement = init_repo_named(&fixture.root, "replacement");
            fixture.state.handle(req("project.update_source", json!({"project_id": fixture.project_id, "source_id": "source-2", "path": replacement})))
        }
        Drift::Base => {
            git_in(&fixture.saved_repo, &["branch", "other"]);
            fixture.state.handle(req("project.update_source", json!({"project_id": fixture.project_id, "source_id": "source-2", "base_branch": "other"})))
        }
    };
    assert_eq!(reply["ok"], true, "{reply}");
}

fn contended_saved_retry(change: Drift) {
    let mut fixture = fixture(false);
    drift(&mut fixture, change);
    let expected = oid(&fixture.saved_repo, "main");
    let shared = fixture.state.shared();
    let (held, _) = SyncLock::acquire(&fixture.saved_repo, Duration::ZERO).unwrap();
    let response = frame_on_a_thread(
        &shared,
        "retry",
        "workspace.retry",
        json!({"workspace_id": fixture.workspace_id}),
    );
    assert!(
        response.recv_timeout(WAITING).is_err(),
        "retry did not wait for its saved source"
    );
    assert!(
        shared.try_lock().is_ok(),
        "retry held the app mutex while waiting"
    );
    let listed = frame_on_a_thread(&shared, "list", "project.list", json!({}))
        .recv_timeout(ANSWERED)
        .unwrap();
    assert_eq!(listed["ok"], true, "{listed}");
    drop(held);
    let reply = response.recv_timeout(ANSWERED).unwrap();
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(oid(&fixture.pending_path, "HEAD"), expected);
    assert_eq!(
        std::fs::read_to_string(&fixture.ready_file).unwrap(),
        "keep completed work"
    );
    assert!(fixture.workspace_root.is_dir());
}

#[test]
fn retry_after_source_removal_waits_for_the_saved_repository() {
    contended_saved_retry(Drift::Remove);
}

#[test]
fn retry_after_source_move_waits_for_the_saved_repository() {
    contended_saved_retry(Drift::Move);
}

#[test]
fn retry_after_base_change_uses_the_saved_oid_after_pinning() {
    let mut fixture = fixture(false);
    drift(&mut fixture, Drift::Base);
    let expected = oid(&fixture.saved_repo, "main");
    let (gate, control) = OffLockGate::new();
    fixture.state.off_lock_workspace_prepared_gate = Some(gate);
    let shared = fixture.state.shared();
    let response = frame_on_a_thread(
        &shared,
        "retry",
        "workspace.retry",
        json!({"workspace_id": fixture.workspace_id}),
    );
    control.wait_for_arrival();
    assert!(shared.try_lock().is_ok());
    std::fs::write(fixture.saved_repo.join("later.txt"), "after pin").unwrap();
    git_in(&fixture.saved_repo, &["add", "."]);
    git_in(&fixture.saved_repo, &["commit", "-q", "-m", "after pin"]);
    assert_ne!(oid(&fixture.saved_repo, "main"), expected);
    control.release();
    let reply = response.recv_timeout(ANSWERED).unwrap();
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(
        oid(&fixture.pending_path, "HEAD"),
        expected,
        "retry resolved the mutable saved base after preparation"
    );
    assert!(!fixture.pending_path.join("later.txt").exists());
    assert_eq!(
        std::fs::read_to_string(&fixture.ready_file).unwrap(),
        "keep completed work"
    );
}

#[test]
fn retry_does_not_repin_an_already_ready_directory() {
    let mut fixture = fixture(true);
    git_in(&fixture.ready_source, &["checkout", "-q", "-b", "other"]);
    git_in(&fixture.ready_source, &["branch", "-D", "main"]);
    let reply = retry(&mut fixture);
    assert_eq!(reply["ok"], true, "{reply}");
    kept(&fixture);
}

#[test]
fn retry_refuses_an_unresolvable_saved_base_without_deleting_ready_files() {
    let mut fixture = fixture(false);
    drift(&mut fixture, Drift::Base);
    git_in(&fixture.saved_repo, &["checkout", "-q", "other"]);
    git_in(&fixture.saved_repo, &["branch", "-D", "main"]);
    let reply = retry(&mut fixture);
    assert_eq!(reply["ok"], false, "{reply}");
    assert!(
        reply.to_string().contains("cannot pin workspace base"),
        "{reply}"
    );
    assert!(!fixture.pending_path.exists());
    kept(&fixture);
}

#[test]
fn retry_refuses_an_inaccessible_saved_repository_without_deleting_ready_files() {
    let mut fixture = fixture(false);
    std::fs::rename(&fixture.saved_repo, fixture.root.join("missing-source")).unwrap();
    let reply = retry(&mut fixture);
    assert_eq!(reply["ok"], false, "{reply}");
    assert!(!fixture.pending_path.exists());
    kept(&fixture);
}
