//! Two workspaces of one project, end to end.
//!
//! A project's workspaces all mount the same source under the same mount name,
//! and the source repository keeps one registry for every checkout cut from it.
//! These tests drive the registry and the façade exactly as
//! `AppState::workspace_create` does, so the second workspace asks the same
//! source for a second record and the name it asks under is what decides
//! whether it gets one.

use super::manager::bare_origin_of;
use crate::git_fixture::init_repo;
use crate::isolation::probe::rift_or_skip;
use crate::isolation::Isolation;
use crate::workspace::{Workspace, WorkspaceRegistry, WorkspaceSource, WorkspaceStatus};
use crate::worktree::WorktreeManager;
use std::path::Path;

/// The one source every workspace here mounts, under the mount name a
/// single-repository project gives it.
fn single_source(repo: &Path) -> Vec<WorkspaceSource> {
    vec![WorkspaceSource {
        id: "source-1".to_string(),
        name: "repo".to_string(),
        mount: "repo".to_string(),
        path: repo.to_path_buf(),
        is_git: true,
        base_branch: "main".to_string(),
    }]
}

/// Create and provision one workspace the way `WorkspaceCreateWork::run` does:
/// a manager per source, rooted at the workspace directory the manifest already
/// names, with the project's own checkouts folder holding the Rift registry.
fn provision(
    registry: &mut WorkspaceRegistry,
    name: &str,
    sources: &[WorkspaceSource],
    isolation: Isolation,
    rift_root: &Path,
) -> Result<Workspace, String> {
    let workspace = registry.begin_with_isolation("proj-1", name, sources, isolation)?;
    registry.provision(
        &workspace.id,
        sources,
        isolation,
        |source, destination, isolation| {
            let manager =
                WorktreeManager::new(&source.path, destination.parent().unwrap_or(destination))
                    .with_rift_registry_root(rift_root);
            let checkout = manager
                .create_workspace_checkout(name, &source.base_branch, destination, isolation)
                .map_err(|error| error.to_string())?;
            Ok((Some(checkout.worktree.recorded_branch), isolation))
        },
    )
}

/// The name each workspace's mount ended up registered under, in workspace
/// order — the pair that has to differ for a project to hold two workspaces.
fn mount_names(workspaces: &[&Workspace]) -> Vec<String> {
    workspaces
        .iter()
        .map(|workspace| {
            crate::isolation::checkout_name(&workspace.directories[0].path)
                .expect("a provisioned mount has a directory")
        })
        .collect()
}

fn git_worktree_entries(repo: &Path) -> Vec<String> {
    let mut entries = std::fs::read_dir(repo.join(".git").join("worktrees"))
        .expect("a project with checkouts has a worktree registry")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    entries.sort();
    entries
}

/// A project's second workspace used to die asking git for a linked-worktree
/// record the first workspace already held, because both mounts were called
/// `repo`. Each mount carries its workspace's directory now, so both
/// materialize and both finish.
#[test]
fn two_workspaces_of_one_project_materialize_and_finish() {
    let (dir, repo) = init_repo();
    // Finishing a workspace verifies its push, so the project has a remote to
    // be finished against.
    bare_origin_of(&dir, &repo);
    let registry_root = dir.path().join("workspaces");
    let rift_root = dir.path().join("checkouts");
    let sources = single_source(&repo);
    let mut registry = WorkspaceRegistry::load(&registry_root).unwrap();

    let first = provision(
        &mut registry,
        "feature",
        &sources,
        Isolation::Worktree,
        &rift_root,
    )
    .unwrap();
    let second = provision(
        &mut registry,
        "feature",
        &sources,
        Isolation::Worktree,
        &rift_root,
    )
    .unwrap();

    assert_eq!(first.status, WorkspaceStatus::Ready);
    assert_eq!(second.status, WorkspaceStatus::Ready);
    assert_eq!(
        mount_names(&[&first, &second]),
        vec!["feature--repo", "feature-2--repo"],
    );
    assert_eq!(
        git_worktree_entries(&repo),
        vec!["feature--repo", "feature-2--repo"],
        "one source repository holds a record per workspace mount",
    );
    for workspace in [&first, &second] {
        assert!(workspace.directories[0].path.join("README.md").is_file());
    }

    for workspace in [first, second] {
        let mut workspace = workspace;
        let finished = registry.finish_existing(&mut workspace).unwrap();
        assert!(
            finished.complete,
            "a provisioned workspace finishes: {finished:?}"
        );
        assert_eq!(workspace.status, WorkspaceStatus::Finished);
    }
}

/// The same project, the same two workspaces, made by Rift. Rift keys its own
/// records by the path it created, so the collision was Build's reading of
/// them: `holds_record` answered for the first workspace's mount when asked
/// about the second's.
#[test]
fn two_rift_workspaces_of_one_project_materialize_and_finish() {
    let (dir, repo) = init_repo();
    if !rift_or_skip(dir.path()) {
        return;
    }
    bare_origin_of(&dir, &repo);
    let registry_root = dir.path().join("workspaces");
    let rift_root = dir.path().join("checkouts");
    let sources = single_source(&repo);
    let mut registry = WorkspaceRegistry::load(&registry_root).unwrap();

    let first = provision(
        &mut registry,
        "feature",
        &sources,
        Isolation::Rift,
        &rift_root,
    )
    .unwrap();
    let second = provision(
        &mut registry,
        "feature",
        &sources,
        Isolation::Rift,
        &rift_root,
    )
    .unwrap();

    assert_eq!(
        mount_names(&[&first, &second]),
        vec!["feature--repo", "feature-2--repo"],
    );
    for workspace in [&first, &second] {
        let mount = &workspace.directories[0].path;
        assert_eq!(Isolation::of(mount), Some(Isolation::Rift));
        assert!(
            mount.file_name().and_then(std::ffi::OsStr::to_str) == Some("repo"),
            "the directory Rift made is still the mount the manifest names",
        );
    }
    let manager =
        WorktreeManager::new(&repo, first.root.clone()).with_rift_registry_root(&rift_root);
    assert!(
        manager.record_held("feature--repo").unwrap(),
        "Rift's record of the first workspace's mount answers to its own name",
    );
    assert!(
        manager.record_held("feature-2--repo").unwrap(),
        "and so does the second's",
    );

    for workspace in [first, second] {
        let mut workspace = workspace;
        assert!(registry.finish_existing(&mut workspace).unwrap().complete);
    }
}

/// The branch each workspace checkout records needs no workspace in its name:
/// it is minted through the project repository's own branch namespace, which
/// is the one place two workspaces of a project meet, so the second workspace
/// is already handed a name the first does not hold.
#[test]
fn each_workspace_checkout_records_a_branch_of_its_own() {
    let (dir, repo) = init_repo();
    let registry_root = dir.path().join("workspaces");
    let rift_root = dir.path().join("checkouts");
    let sources = single_source(&repo);
    let mut registry = WorkspaceRegistry::load(&registry_root).unwrap();

    let first = provision(
        &mut registry,
        "feature",
        &sources,
        Isolation::Worktree,
        &rift_root,
    )
    .unwrap();
    let second = provision(
        &mut registry,
        "feature",
        &sources,
        Isolation::Worktree,
        &rift_root,
    )
    .unwrap();

    assert_eq!(
        first.directories[0].branch.as_deref(),
        Some("build/feature")
    );
    assert_eq!(
        second.directories[0].branch.as_deref(),
        Some("build/feature-2"),
    );
}
