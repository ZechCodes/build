use super::*;
use std::collections::BTreeSet;

fn keys(registry: &WorkspaceRegistry) -> BTreeSet<String> {
    registry
        .observation_changes()
        .snapshot()
        .into_iter()
        .map(|(key, _)| key)
        .collect()
}

fn acknowledge(registry: &WorkspaceRegistry) {
    let captured = registry.observation_changes().snapshot();
    registry.observation_changes().acknowledge(&captured);
}

fn managed(registry: &mut WorkspaceRegistry, name: &str) -> Workspace {
    registry.begin("project", name, &[]).unwrap()
}

fn finishable_repository(root: &Path, name: &str) -> PathBuf {
    let repository = crate::git_fixture::init_repo_named(root, name);
    let origin = root.join(format!("{name}-origin.git"));
    crate::git_fixture::git_in(
        root,
        &["init", "--bare", "-b", "main", origin.to_str().unwrap()],
    );
    crate::git_fixture::configure_repo(&origin);
    crate::git_fixture::git_in(
        &repository,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    crate::git_fixture::git_in(&repository, &["push", "--set-upstream", "origin", "main"]);
    repository
}

#[test]
fn workspace_changes_keep_scoped_revisions_and_ignore_failed_or_identical_saves() {
    let temp = tempfile::tempdir().unwrap();
    let mut registry = WorkspaceRegistry::empty(temp.path().join("workspaces"));
    let first = managed(&mut registry, "first");
    let second = managed(&mut registry, "second");
    acknowledge(&registry);
    registry.rename(&first.id, "first").unwrap();
    registry.set_locked(&second.id, false).unwrap();
    assert!(keys(&registry).is_empty());

    let manifest = first.root.join(MANIFEST_FILE);
    let backup = first.root.join("manifest-backup");
    fs::rename(&manifest, &backup).unwrap();
    fs::create_dir(&manifest).unwrap();
    assert!(registry.rename(&first.id, "unsaved").is_err());
    assert_eq!(registry.get(&first.id).unwrap(), &first);
    assert!(keys(&registry).is_empty());
    fs::remove_dir(&manifest).unwrap();
    fs::rename(&backup, &manifest).unwrap();

    registry.rename(&first.id, "saved").unwrap();
    let captured = registry.observation_changes().snapshot();
    registry.rename(&first.id, "newer saved").unwrap();
    registry.observation_changes().acknowledge(&captured);
    assert_eq!(keys(&registry), BTreeSet::from([first.id.clone()]));
    let newer = registry.observation_changes().snapshot();
    assert!(newer[0].1 > captured[0].1);
    registry.observation_changes().acknowledge(&newer);
    assert!(keys(&registry).is_empty());
}

#[test]
fn workspace_reload_marks_only_real_changes_and_preserves_rows_after_invalid_manifest() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("workspaces");
    let mut registry = WorkspaceRegistry::empty(&root);
    let first = managed(&mut registry, "first");
    let second = managed(&mut registry, "second");
    acknowledge(&registry);
    registry.reload().unwrap();
    assert!(keys(&registry).is_empty());

    let mut changed = first.clone();
    changed.name = "worker-saved".into();
    persist(&changed).unwrap();
    fs::remove_dir_all(&second.root).unwrap();
    let mut worker = WorkspaceRegistry::load(&root).unwrap();
    let third = managed(&mut worker, "third");
    registry.reload().unwrap();
    assert_eq!(
        keys(&registry),
        BTreeSet::from([first.id.clone(), second.id, third.id])
    );
    assert_eq!(registry.get(&first.id).unwrap(), &changed);
    acknowledge(&registry);

    fs::write(first.root.join(MANIFEST_FILE), b"not a manifest").unwrap();
    assert!(registry.reload().is_err());
    assert_eq!(registry.get(&first.id).unwrap(), &changed);
    assert!(keys(&registry).is_empty());
}

#[test]
fn adopted_finish_reopen_and_capability_refresh_mark_only_changed_workspaces() {
    let temp = tempfile::tempdir().unwrap();
    let mut registry = WorkspaceRegistry::empty(temp.path().join("workspaces"));
    let adopted = finishable_repository(temp.path(), "adopted");
    let untouched = temp.path().join("untouched");
    fs::create_dir(&untouched).unwrap();
    let mut workspace = registry.adopt_root(
        "project",
        "adopted".into(),
        "adopted".into(),
        adopted,
        "source".into(),
        true,
    );
    registry.adopt_root(
        "project",
        "untouched".into(),
        "untouched".into(),
        untouched.clone(),
        "source".into(),
        false,
    );
    acknowledge(&registry);
    registry.refresh_local_capabilities();
    registry.refresh_finished_local();
    assert!(keys(&registry).is_empty());

    registry.record_finished(&mut workspace).unwrap();
    assert_eq!(keys(&registry), BTreeSet::from([workspace.id.clone()]));
    registry.reload().unwrap();
    acknowledge(&registry);
    registry.reopen(&workspace.id).unwrap();
    assert_eq!(
        registry.get(&workspace.id).unwrap().status,
        WorkspaceStatus::Ready
    );
    assert_eq!(keys(&registry), BTreeSet::from([workspace.id.clone()]));
    acknowledge(&registry);
    registry.reopen(&workspace.id).unwrap();
    assert!(keys(&registry).is_empty());

    git2::Repository::init(&untouched).unwrap();
    registry.refresh_local_capabilities();
    assert_eq!(keys(&registry), BTreeSet::from(["untouched".into()]));
    assert!(registry.get("untouched").unwrap().directories[0].is_git);
    acknowledge(&registry);
    registry.refresh_local_capabilities();
    assert!(keys(&registry).is_empty());
}

#[test]
fn failed_reopen_preserves_committed_status_and_its_change_revision() {
    let temp = tempfile::tempdir().unwrap();
    let mut registry = WorkspaceRegistry::empty(temp.path().join("workspaces"));
    let workspace = managed(&mut registry, "finished");
    let mut finished = workspace.clone();
    finished.status = WorkspaceStatus::Finished;
    persist(&finished).unwrap();
    registry.reload().unwrap();
    acknowledge(&registry);
    let manifest = workspace.root.join(MANIFEST_FILE);
    fs::remove_file(&manifest).unwrap();
    fs::create_dir(&manifest).unwrap();
    assert!(registry.reopen(&workspace.id).is_err());
    assert_eq!(registry.get(&workspace.id).unwrap(), &finished);
    assert!(keys(&registry).is_empty());
}

#[test]
fn missing_registry_root_retires_managed_rows_and_keeps_adopted_legacy_state() {
    let temp = tempfile::tempdir().unwrap();
    let mut registry = WorkspaceRegistry::empty(temp.path().join("workspaces"));
    let managed = managed(&mut registry, "removed");
    let external = finishable_repository(temp.path(), "external");
    let mut adopted = registry.adopt_root(
        "project",
        "adopted".into(),
        "adopted".into(),
        external,
        "source".into(),
        true,
    );
    registry.record_finished(&mut adopted).unwrap();
    registry.reload().unwrap();
    assert_eq!(
        registry.get(&adopted.id).unwrap().status,
        WorkspaceStatus::Finished
    );
    acknowledge(&registry);
    fs::remove_dir_all(registry.root()).unwrap();
    registry.reload().unwrap();
    assert!(registry.get(&managed.id).is_none());
    assert_eq!(
        registry.get(&adopted.id).unwrap().status,
        WorkspaceStatus::Finished
    );
    assert_eq!(keys(&registry), BTreeSet::from([managed.id]));
    acknowledge(&registry);
    registry.reload().unwrap();
    assert!(keys(&registry).is_empty());
}
