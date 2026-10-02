use super::model::{
    ReviewBase, ReviewBaseKind, ReviewDirectory, ReviewDirectoryStatus, ReviewSnapshot,
};
use crate::tracker::Actor;
use crate::workspace::{DirectoryStatus, Workspace, WorkspaceDirectory};
use std::collections::BTreeMap;

pub fn capture(
    task_id: &str,
    snapshot_id: &str,
    workspace: &Workspace,
    base_overrides: &BTreeMap<String, String>,
    author: &Actor,
) -> Result<ReviewSnapshot, String> {
    for directory_id in base_overrides.keys() {
        if !workspace.directories.iter().any(|directory| {
            &directory.id == directory_id
                && directory.is_git
                && directory.status == DirectoryStatus::Ready
        }) {
            return Err(format!("unknown directory override: {directory_id}"));
        }
    }
    let mut snapshot = ReviewSnapshot {
        id: snapshot_id.into(),
        number: 0,
        created_at: time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|error| error.to_string())?,
        author: author.clone(),
        directories: Vec::with_capacity(workspace.directories.len()),
    };
    for directory in &workspace.directories {
        let mut saved = ReviewDirectory::from(directory);
        if saved.status == ReviewDirectoryStatus::Git {
            match capture_git(
                task_id,
                snapshot_id,
                directory,
                base_overrides.get(&directory.id),
                &mut saved,
            ) {
                Err(CaptureGitError::InvalidOverride(error)) => {
                    let _ = cleanup_pins(task_id, &snapshot);
                    let _ = cleanup_directory_pins(task_id, snapshot_id, &saved);
                    return Err(format!(
                        "invalid base override for {}: {error}",
                        directory.id
                    ));
                }
                Err(CaptureGitError::Unavailable(error)) => {
                    saved.status = ReviewDirectoryStatus::Unavailable;
                    saved.reason = Some(error);
                }
                Ok(()) => {}
            }
        }
        snapshot.directories.push(saved);
    }
    Ok(snapshot)
}

impl From<&WorkspaceDirectory> for ReviewDirectory {
    fn from(directory: &WorkspaceDirectory) -> Self {
        let status = if directory.status != DirectoryStatus::Ready || !directory.path.is_dir() {
            ReviewDirectoryStatus::Unavailable
        } else if directory.is_git {
            ReviewDirectoryStatus::Git
        } else {
            ReviewDirectoryStatus::NotGit
        };
        let reason = if status == ReviewDirectoryStatus::Unavailable {
            Some(
                directory
                    .error
                    .clone()
                    .unwrap_or_else(|| "source unavailable".into()),
            )
        } else {
            None
        };
        Self {
            id: directory.id.clone(),
            source_id: directory.source_id.clone(),
            name: directory.name.clone(),
            path: directory.path.clone(),
            source_path: directory.source_path.clone(),
            is_git: directory.is_git,
            status,
            reason,
            common_git_dir: None,
            branch: None,
            base: None,
            head: None,
            uncommitted_files: None,
        }
    }
}

enum CaptureGitError {
    InvalidOverride(String),
    Unavailable(String),
}

impl From<String> for CaptureGitError {
    fn from(error: String) -> Self {
        Self::Unavailable(error)
    }
}

fn capture_git(
    task_id: &str,
    snapshot_id: &str,
    directory: &WorkspaceDirectory,
    override_name: Option<&String>,
    saved: &mut ReviewDirectory,
) -> Result<(), CaptureGitError> {
    if !directory.path.is_dir() {
        return Err(CaptureGitError::Unavailable("source unavailable".into()));
    }
    let repo = git2::Repository::open(&directory.path).map_err(|error| error.to_string())?;
    if repo.workdir().and_then(|path| path.canonicalize().ok())
        != directory.path.canonicalize().ok()
    {
        return Err(CaptureGitError::Unavailable(
            "source is not the repository root".into(),
        ));
    }
    saved.common_git_dir = Some(
        repo.commondir()
            .canonicalize()
            .map_err(|error| error.to_string())?,
    );
    let head = match repo.head() {
        Ok(head) => Some(head),
        Err(error)
            if matches!(
                error.code(),
                git2::ErrorCode::UnbornBranch | git2::ErrorCode::NotFound
            ) =>
        {
            None
        }
        Err(error) => return Err(CaptureGitError::Unavailable(error.to_string())),
    };
    saved.branch = head
        .as_ref()
        .and_then(|head| {
            head.is_branch()
                .then(|| head.shorthand())
                .flatten()
                .map(str::to_owned)
        })
        .or_else(|| {
            repo.find_reference("HEAD").ok().and_then(|head| {
                head.symbolic_target()
                    .and_then(|name| name.strip_prefix("refs/heads/"))
                    .map(str::to_owned)
            })
        });
    let head_oid = head.as_ref().and_then(git2::Reference::target);
    saved.uncommitted_files = Some(count_uncommitted(&repo)?);
    let base = resolve_base(&repo, directory, override_name).map_err(|error| {
        if override_name.is_some() {
            CaptureGitError::InvalidOverride(error)
        } else {
            CaptureGitError::Unavailable(error)
        }
    })?;
    let prefix = pin_prefix(task_id, snapshot_id, &directory.id)?;
    if let Some(head_oid) = head_oid {
        create_pin(&repo, &format!("{prefix}/head"), head_oid)?;
        saved.head = Some(head_oid.to_string());
    } else {
        saved.status = ReviewDirectoryStatus::NoCommits;
    }
    if base.kind != ReviewBaseKind::EmptyTree {
        let base_oid = git2::Oid::from_str(&base.oid).map_err(|error| error.to_string())?;
        if let Err(error) = create_pin(&repo, &format!("{prefix}/base"), base_oid) {
            if delete_pin_if_expected(&repo, &format!("{prefix}/head"), saved.head.as_deref())
                .is_ok()
            {
                saved.head = None;
            }
            return Err(CaptureGitError::Unavailable(error));
        }
    }
    saved.base = Some(base);
    Ok(())
}

fn count_uncommitted(repo: &git2::Repository) -> Result<u64, String> {
    let mut options = git2::StatusOptions::new();
    options.include_untracked(true).recurse_untracked_dirs(true);
    let statuses = repo
        .statuses(Some(&mut options))
        .map_err(|error| error.to_string())?;
    Ok(statuses
        .iter()
        .filter(|entry| {
            entry
                .path()
                .is_some_and(|path| !crate::diff::is_mcp_config(path))
        })
        .count() as u64)
}

fn resolve_base(
    repo: &git2::Repository,
    directory: &WorkspaceDirectory,
    override_name: Option<&String>,
) -> Result<ReviewBase, String> {
    if let Some(name) = override_name {
        let oid = resolve_commit(repo, name)?;
        return Ok(ReviewBase {
            kind: ReviewBaseKind::Override,
            name: Some(name.clone()),
            oid: oid.to_string(),
        });
    }
    if let Ok(oid) = resolve_commit(repo, &directory.base_branch) {
        return Ok(ReviewBase {
            kind: ReviewBaseKind::Configured,
            name: Some(directory.base_branch.clone()),
            oid: oid.to_string(),
        });
    }
    if let Ok(head) = repo.head() {
        if let Some(name) = head
            .shorthand()
            .and_then(|branch| repo.find_branch(branch, git2::BranchType::Local).ok())
            .and_then(|branch| branch.upstream().ok())
            .and_then(|branch| branch.get().name().map(str::to_owned))
        {
            if let Ok(oid) = resolve_commit(repo, &name) {
                return Ok(ReviewBase {
                    kind: ReviewBaseKind::Upstream,
                    name: Some(name),
                    oid: oid.to_string(),
                });
            }
        }
    }
    let oid = repo
        .treebuilder(None)
        .and_then(|tree| tree.write())
        .map_err(|error| error.to_string())?;
    Ok(ReviewBase {
        kind: ReviewBaseKind::EmptyTree,
        name: None,
        oid: oid.to_string(),
    })
}

fn resolve_commit(repo: &git2::Repository, name: &str) -> Result<git2::Oid, String> {
    repo.revparse_single(name)
        .and_then(|object| object.peel_to_commit())
        .map(|commit| commit.id())
        .map_err(|error| error.to_string())
}

fn pin_prefix(task_id: &str, snapshot_id: &str, directory_id: &str) -> Result<String, String> {
    if [task_id, snapshot_id, directory_id].contains(&"") {
        return Err("invalid review pin identity".into());
    }
    let prefix = format!(
        "refs/build/reviews/{}/{}/{}",
        encode_pin_component(task_id),
        encode_pin_component(snapshot_id),
        encode_pin_component(directory_id)
    );
    if !git2::Reference::is_valid_name(&format!("{prefix}/head")) {
        return Err("invalid review pin identity".into());
    }
    Ok(prefix)
}

/// Workspace directory IDs contain `:`; ref components cannot. Escape every
/// non-alphanumeric byte, including `%`, so distinct IDs stay distinct.
fn encode_pin_component(component: &str) -> String {
    let mut encoded = String::with_capacity(component.len());
    for byte in component.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn create_pin(repo: &git2::Repository, name: &str, oid: git2::Oid) -> Result<(), String> {
    repo.reference(name, oid, false, "Build review snapshot")
        .map(|_| ())
        .map_err(|error| error.to_string())
}

pub fn cleanup_pins(task_id: &str, snapshot: &ReviewSnapshot) -> Result<(), String> {
    for directory in &snapshot.directories {
        cleanup_directory_pins(task_id, &snapshot.id, directory)?;
    }
    Ok(())
}

fn cleanup_directory_pins(
    task_id: &str,
    snapshot_id: &str,
    directory: &ReviewDirectory,
) -> Result<(), String> {
    let Some(common_dir) = directory.common_git_dir.as_ref() else {
        return Ok(());
    };
    if !common_dir.exists() {
        return Ok(());
    }
    let repo = git2::Repository::open_bare(common_dir).map_err(|error| error.to_string())?;
    let prefix = pin_prefix(task_id, snapshot_id, &directory.id)?;
    delete_pin_if_expected(&repo, &format!("{prefix}/head"), directory.head.as_deref())?;
    delete_pin_if_expected(
        &repo,
        &format!("{prefix}/base"),
        directory.base.as_ref().map(|base| base.oid.as_str()),
    )?;
    Ok(())
}

fn delete_pin_if_expected(
    repo: &git2::Repository,
    name: &str,
    expected: Option<&str>,
) -> Result<(), String> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let mut transaction = repo.transaction().map_err(|error| error.to_string())?;
    transaction
        .lock_ref(name)
        .map_err(|error| error.to_string())?;
    let reference = match repo.find_reference(name) {
        Ok(reference) => reference,
        Err(error) if error.code() == git2::ErrorCode::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    if !reference
        .target()
        .is_some_and(|oid| oid.to_string() == expected)
    {
        return Err(format!("review pin changed: {name}"));
    }
    transaction
        .remove(name)
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_fixture::{git_command, git_in, init_repo};
    use crate::workspace::{DirectoryStatus, WorkspaceDirectory, WorkspaceStatus};

    fn workspace(repo: &std::path::Path) -> Workspace {
        Workspace {
            id: "workspace-1".into(),
            project_id: "project-1".into(),
            name: "review fixture".into(),
            root: repo.parent().unwrap().to_path_buf(),
            status: WorkspaceStatus::Ready,
            archived_at: None,
            directories: vec![WorkspaceDirectory {
                id: "directory-1".into(),
                source_id: "source-1".into(),
                name: "repo".into(),
                path: repo.to_path_buf(),
                is_git: true,
                branch: Some("main".into()),
                effective_isolation: None,
                finished_head: None,
                status: DirectoryStatus::Ready,
                source_path: repo.to_path_buf(),
                base_branch: "main".into(),
                error: None,
            }],
            isolation: Default::default(),
            managed: false,
            created_by_agent: false,
        }
    }

    #[test]
    fn snapshot_pins_head_and_keeps_dirty_files_out() {
        let (_temp, repo) = init_repo();
        git_in(&repo, &["checkout", "-b", "feature"]);
        std::fs::write(repo.join("feature.txt"), "committed\n").unwrap();
        git_in(&repo, &["add", "feature.txt"]);
        git_in(&repo, &["commit", "-m", "feature"]);
        std::fs::write(repo.join("dirty.txt"), "uncommitted\n").unwrap();
        let snapshot = capture(
            "task-1",
            "snapshot-1",
            &workspace(&repo),
            &BTreeMap::new(),
            &Actor::User,
        )
        .unwrap();
        let directory = &snapshot.directories[0];
        assert_eq!(directory.branch.as_deref(), Some("feature"));
        assert_eq!(directory.uncommitted_files, Some(1));
        assert_eq!(
            directory.base.as_ref().unwrap().name.as_deref(),
            Some("main")
        );
        let pinned = git_command(
            &repo,
            &[
                "rev-parse",
                "refs/build/reviews/task-1/snapshot-1/directory-1/head",
            ],
        )
        .output()
        .unwrap();
        assert!(pinned.status.success());
        assert_eq!(
            String::from_utf8(pinned.stdout).unwrap().trim(),
            directory.head.as_ref().unwrap()
        );
        std::fs::remove_file(repo.join("dirty.txt")).unwrap();
        git_in(&repo, &["checkout", "main"]);
        git_in(&repo, &["branch", "-D", "feature"]);
        git_in(&repo, &["gc", "--prune=now"]);
        let read = crate::reviews::read::read(
            directory,
            &crate::reviews::read::ReviewReadRequest {
                mode: crate::reviews::read::ReviewReadMode::Blob,
                path: Some("feature.txt".into()),
                paths: Vec::new(),
                range: None,
                patch: true,
            },
        )
        .unwrap();
        let crate::reviews::read::ReviewReadResult::Blob(blob) = read else {
            panic!("blob expected")
        };
        assert_eq!(blob.content_b64, crate::encoding::b64encode(b"committed\n"));
        cleanup_pins("task-1", &snapshot).unwrap();
    }

    #[test]
    fn invalid_override_leaves_no_refs() {
        let (_temp, repo) = init_repo();
        let overrides = BTreeMap::from([("directory-1".into(), "missing-branch".into())]);
        assert!(capture(
            "task-1",
            "snapshot-2",
            &workspace(&repo),
            &overrides,
            &Actor::User
        )
        .is_err());
        let pin = git_command(
            &repo,
            &[
                "show-ref",
                "--verify",
                "refs/build/reviews/task-1/snapshot-2/directory-1/head",
            ],
        )
        .output()
        .unwrap();
        assert!(!pin.status.success());
    }

    #[test]
    fn failed_pin_does_not_delete_a_preexisting_ref_with_the_same_oid() {
        let (_temp, repo) = init_repo();
        let base_ref = "refs/build/reviews/task-1/collision/directory-1/base";
        git_in(&repo, &["update-ref", base_ref, "HEAD"]);
        let snapshot = capture(
            "task-1",
            "collision",
            &workspace(&repo),
            &BTreeMap::new(),
            &Actor::User,
        )
        .unwrap();
        assert_eq!(
            snapshot.directories[0].status,
            ReviewDirectoryStatus::Unavailable
        );
        assert!(git_command(&repo, &["show-ref", "--verify", base_ref])
            .output()
            .unwrap()
            .status
            .success());
        assert!(!git_command(
            &repo,
            &[
                "show-ref",
                "--verify",
                "refs/build/reviews/task-1/collision/directory-1/head"
            ]
        )
        .output()
        .unwrap()
        .status
        .success());
        cleanup_pins("task-1", &snapshot).unwrap();
        assert!(git_command(&repo, &["show-ref", "--verify", base_ref])
            .output()
            .unwrap()
            .status
            .success());
    }

    #[test]
    fn unborn_repository_keeps_its_working_files_out_of_review() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        git_in(&repo, &["init", "-b", "main"]);
        crate::git_fixture::configure_repo(&repo);
        std::fs::write(repo.join("untracked.txt"), "not committed\n").unwrap();
        let snapshot = capture(
            "task-1",
            "snapshot-3",
            &workspace(&repo),
            &BTreeMap::new(),
            &Actor::User,
        )
        .unwrap();
        let directory = &snapshot.directories[0];
        assert_eq!(directory.status, ReviewDirectoryStatus::NoCommits);
        assert_eq!(directory.head, None);
        assert_eq!(directory.branch.as_deref(), Some("main"));
        assert_eq!(directory.uncommitted_files, Some(1));
        assert_eq!(
            directory.base.as_ref().unwrap().kind,
            ReviewBaseKind::EmptyTree
        );
    }

    #[test]
    fn unborn_head_still_pins_a_resolved_base() {
        let (_temp, repo) = init_repo();
        git_in(&repo, &["checkout", "--orphan", "unborn"]);
        let snapshot = capture(
            "task-1",
            "unborn-base",
            &workspace(&repo),
            &BTreeMap::new(),
            &Actor::User,
        )
        .unwrap();
        let directory = &snapshot.directories[0];
        assert_eq!(directory.status, ReviewDirectoryStatus::NoCommits);
        assert!(directory.head.is_none());
        assert_eq!(
            directory.base.as_ref().unwrap().kind,
            ReviewBaseKind::Configured
        );
        let pin = git_command(
            &repo,
            &[
                "show-ref",
                "--verify",
                "refs/build/reviews/task-1/unborn-base/directory-1/base",
            ],
        )
        .output()
        .unwrap();
        assert!(pin.status.success());
        cleanup_pins("task-1", &snapshot).unwrap();
    }

    #[test]
    fn shared_repository_directories_get_distinct_refs_and_cleanup_checks_the_oid() {
        let (_temp, repo) = init_repo();
        let mut workspace = workspace(&repo);
        let mut second = workspace.directories[0].clone();
        second.id = "directory-2".into();
        second.source_id = "source-2".into();
        workspace.directories.push(second);
        let snapshot = capture(
            "task-1",
            "snapshot-4",
            &workspace,
            &BTreeMap::new(),
            &Actor::User,
        )
        .unwrap();
        assert_eq!(snapshot.directories.len(), 2);
        for directory in &snapshot.directories {
            let name = format!("refs/build/reviews/task-1/snapshot-4/{}/head", directory.id);
            assert!(git_command(&repo, &["show-ref", "--verify", &name])
                .output()
                .unwrap()
                .status
                .success());
        }
        std::fs::write(repo.join("later.txt"), "later\n").unwrap();
        git_in(&repo, &["add", "later.txt"]);
        git_in(&repo, &["commit", "-m", "later"]);
        git_in(
            &repo,
            &[
                "update-ref",
                "refs/build/reviews/task-1/snapshot-4/directory-2/head",
                "HEAD",
            ],
        );
        assert!(cleanup_pins("task-1", &snapshot).is_err());
        assert!(!git_command(
            &repo,
            &[
                "show-ref",
                "--verify",
                "refs/build/reviews/task-1/snapshot-4/directory-1/head"
            ]
        )
        .output()
        .unwrap()
        .status
        .success());
        assert!(git_command(
            &repo,
            &[
                "show-ref",
                "--verify",
                "refs/build/reviews/task-1/snapshot-4/directory-2/head"
            ]
        )
        .output()
        .unwrap()
        .status
        .success());
    }

    #[test]
    fn real_workspace_ids_are_escaped_without_pin_collisions() {
        let (_temp, repo) = init_repo();
        let mut workspace = workspace(&repo);
        workspace.directories[0].id = "workspace-id:source-1".into();
        let mut second = workspace.directories[0].clone();
        second.id = "workspace-id%3Asource-1".into();
        second.source_id = "source-2".into();
        workspace.directories.push(second);
        let snapshot = capture(
            "task",
            "snapshot",
            &workspace,
            &BTreeMap::new(),
            &Actor::User,
        )
        .unwrap();
        assert!(snapshot
            .directories
            .iter()
            .all(|directory| directory.status == ReviewDirectoryStatus::Git));
        for name in [
            "refs/build/reviews/task/snapshot/workspace-id%3Asource-1/head",
            "refs/build/reviews/task/snapshot/workspace-id%253Asource-1/head",
        ] {
            assert!(git_command(&repo, &["show-ref", "--verify", name])
                .output()
                .unwrap()
                .status
                .success());
        }
        cleanup_pins("task", &snapshot).unwrap();
    }

    #[test]
    fn base_override_upstream_empty_tree_and_detached_head_are_recorded() {
        let (_temp, repo) = init_repo();
        git_in(&repo, &["remote", "add", "origin", repo.to_str().unwrap()]);
        git_in(&repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        git_in(&repo, &["checkout", "-b", "feature"]);
        git_in(
            &repo,
            &["branch", "--set-upstream-to", "origin/main", "feature"],
        );
        let mut workspace = workspace(&repo);
        workspace.directories[0].base_branch = "missing".into();
        let upstream = capture(
            "task",
            "upstream",
            &workspace,
            &BTreeMap::new(),
            &Actor::User,
        )
        .unwrap();
        assert_eq!(
            upstream.directories[0].base.as_ref().unwrap().kind,
            ReviewBaseKind::Upstream
        );
        assert_eq!(
            upstream.directories[0]
                .base
                .as_ref()
                .unwrap()
                .name
                .as_deref(),
            Some("refs/remotes/origin/main")
        );
        let overrides = BTreeMap::from([("directory-1".into(), "main".into())]);
        let overridden = capture("task", "override", &workspace, &overrides, &Actor::User).unwrap();
        assert_eq!(
            overridden.directories[0].base.as_ref().unwrap().kind,
            ReviewBaseKind::Override
        );
        git_in(&repo, &["branch", "--unset-upstream"]);
        let empty = capture("task", "empty", &workspace, &BTreeMap::new(), &Actor::User).unwrap();
        assert_eq!(
            empty.directories[0].base.as_ref().unwrap().kind,
            ReviewBaseKind::EmptyTree
        );
        let changes = crate::reviews::read::read(
            &empty.directories[0],
            &crate::reviews::read::ReviewReadRequest {
                mode: crate::reviews::read::ReviewReadMode::Changes,
                path: None,
                paths: Vec::new(),
                range: None,
                patch: true,
            },
        )
        .unwrap();
        let crate::reviews::read::ReviewReadResult::Changes(changes) = changes else {
            panic!("changes expected")
        };
        assert_eq!(changes.files[0].path, "README.md");
        git_in(&repo, &["checkout", "--detach"]);
        workspace.directories[0].base_branch = "main".into();
        let detached = capture(
            "task",
            "detached",
            &workspace,
            &BTreeMap::new(),
            &Actor::User,
        )
        .unwrap();
        assert_eq!(detached.directories[0].branch, None);
        assert!(detached.directories[0].head.is_some());
    }

    #[test]
    fn nonancestor_override_base_is_pinned_after_branch_deletion() {
        let (_temp, repo) = init_repo();
        git_in(&repo, &["checkout", "-b", "other"]);
        std::fs::write(repo.join("other.txt"), "other\n").unwrap();
        git_in(&repo, &["add", "other.txt"]);
        git_in(&repo, &["commit", "-m", "other"]);
        git_in(&repo, &["checkout", "main"]);
        git_in(&repo, &["checkout", "-b", "feature"]);
        std::fs::write(repo.join("feature.txt"), "feature\n").unwrap();
        git_in(&repo, &["add", "feature.txt"]);
        git_in(&repo, &["commit", "-m", "feature"]);
        let overrides = BTreeMap::from([("directory-1".into(), "other".into())]);
        let snapshot = capture(
            "task",
            "nonancestor",
            &workspace(&repo),
            &overrides,
            &Actor::User,
        )
        .unwrap();
        git_in(&repo, &["branch", "-D", "other"]);
        git_in(&repo, &["reflog", "expire", "--expire=now", "--all"]);
        git_in(&repo, &["gc", "--prune=now"]);
        let changes = crate::reviews::read::read(
            &snapshot.directories[0],
            &crate::reviews::read::ReviewReadRequest {
                mode: crate::reviews::read::ReviewReadMode::Changes,
                path: None,
                paths: Vec::new(),
                range: None,
                patch: true,
            },
        )
        .unwrap();
        let crate::reviews::read::ReviewReadResult::Changes(changes) = changes else {
            panic!("changes expected")
        };
        assert_eq!(
            changes
                .files
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            ["feature.txt", "other.txt"]
        );
    }

    #[test]
    fn snapshot_keeps_manifest_order_with_non_git_and_unavailable_directories() {
        let (temp, repo) = init_repo();
        let plain = temp.path().join("plain");
        std::fs::create_dir(&plain).unwrap();
        let mut workspace = workspace(&repo);
        let mut non_git = workspace.directories[0].clone();
        non_git.id = "plain".into();
        non_git.source_id = "plain-source".into();
        non_git.name = "plain".into();
        non_git.path = plain.clone();
        non_git.source_path = plain;
        non_git.is_git = false;
        let mut missing = workspace.directories[0].clone();
        missing.id = "missing".into();
        missing.path = temp.path().join("missing");
        missing.error = Some("removed".into());
        let mut later = workspace.directories[0].clone();
        later.id = "later".into();
        workspace.directories.extend([non_git, missing, later]);
        let overrides = BTreeMap::from([("missing".into(), "cannot-resolve".into())]);
        let snapshot = capture("task", "all", &workspace, &overrides, &Actor::User).unwrap();
        assert_eq!(
            snapshot
                .directories
                .iter()
                .map(|directory| directory.id.as_str())
                .collect::<Vec<_>>(),
            ["directory-1", "plain", "missing", "later"]
        );
        assert_eq!(
            snapshot.directories[1].status,
            ReviewDirectoryStatus::NotGit
        );
        assert!(!snapshot.directories[1].is_git);
        assert_eq!(
            snapshot.directories[2].status,
            ReviewDirectoryStatus::Unavailable
        );
        assert_eq!(snapshot.directories[2].reason.as_deref(), Some("removed"));
        assert_eq!(snapshot.directories[3].status, ReviewDirectoryStatus::Git);
    }
}
