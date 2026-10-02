//! Explicit Git steps for a saved review source.

use super::actions::ReviewRemote;
use super::model::ReviewDirectory;
use crate::git_process::{
    git_failure, run_git_unattended, run_git_unattended_observed, GitProcessEvent,
};
use crate::isolation::worktree::{
    is_owned_review_target, mark_review_target_launch, materialize_review_target,
    record_review_target_process, recover_review_target, remove_review_target,
};
use crate::remote_url::usable_remote_url;
use crate::source_sync::unfinished_on_branch;
use serde::Serialize;
use std::ffi::OsStr;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GitDestinations {
    pub branches: Vec<String>,
    pub remotes: Vec<ReviewRemote>,
    pub live_head: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GitStepOutcome {
    pub head: String,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitActionError {
    Failed(String),
    TimedOut(String),
    TimedOutWithOwnedLock(String, ObservedLock),
    OutcomeUnknown(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObservedLock {
    dev: u64,
    ino: u64,
}

#[cfg(unix)]
fn lock_identity(path: &Path) -> Option<ObservedLock> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(path).ok()?;
    Some(ObservedLock {
        dev: metadata.dev(),
        ino: metadata.ino(),
    })
}

#[cfg(not(unix))]
fn lock_identity(_path: &Path) -> Option<ObservedLock> {
    None
}

fn child_holds_index_lock(pid: u32, lock: &Path) -> Option<ObservedLock> {
    let identity = lock_identity(lock)?;
    let fds = std::fs::read_dir(format!("/proc/{pid}/fd")).ok()?;
    fds.filter_map(Result::ok)
        .any(|fd| lock_identity(&fd.path()) == Some(identity))
        .then_some(identity)
}

impl std::fmt::Display for GitActionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Failed(message)
            | Self::TimedOut(message)
            | Self::OutcomeUnknown(message)
            | Self::TimedOutWithOwnedLock(message, _) => formatter.write_str(message),
        }
    }
}

/// Sweep Build-owned temporary review checkouts after interrupted actions.
pub fn recover_temporary_worktrees(source: &Path) -> Result<usize, String> {
    let mut removed = 0;
    for (path, _) in worktree_placements(source)? {
        if !looks_like_review_target(&path) && !is_owned_review_target(source, &path) {
            continue;
        }
        recover_review_target(source, &path)?;
        if let Some(parent) = path.parent() {
            let _ = std::fs::remove_dir(parent);
        }
        removed += 1;
    }
    Ok(removed)
}

pub fn destinations(
    directory: &ReviewDirectory,
    source_path: &Path,
) -> Result<GitDestinations, String> {
    source_repo(directory, source_path)?;
    let mut branches = git(
        source_path,
        &["for-each-ref", "--format=%(refname:strip=2)", "refs/heads"],
    )?
    .lines()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    branches.sort();
    let mut remotes = Vec::new();
    for name in git(source_path, &["remote"])?.lines() {
        valid_remote(source_path, name)?;
        let prefix = format!("refs/remotes/{name}");
        let mut remote_branches = git(
            source_path,
            &["for-each-ref", "--format=%(refname)", &prefix],
        )?
        .lines()
        .filter_map(|reference| reference.strip_prefix(&(prefix.clone() + "/")))
        .filter(|branch| *branch != "HEAD")
        .map(str::to_owned)
        .collect::<Vec<_>>();
        remote_branches.sort();
        remotes.push(ReviewRemote {
            name: name.into(),
            branches: remote_branches,
        });
    }
    let live_head = git2::Repository::open(&directory.path)
        .ok()
        .and_then(|repo| {
            repo.head()
                .ok()
                .and_then(|head| head.target().map(|oid| oid.to_string()))
        });
    Ok(GitDestinations {
        branches,
        remotes,
        live_head,
    })
}

pub fn merge(
    directory: &ReviewDirectory,
    source_path: &Path,
    target_branch: &str,
) -> Result<GitStepOutcome, String> {
    source_repo(directory, source_path)?;
    valid_branch(source_path, target_branch)?;
    let target_ref = format!("refs/heads/{target_branch}");
    git(source_path, &["show-ref", "--verify", &target_ref])?;
    let head = saved_head(directory)?;
    let imported = import_if_needed(directory, source_path, head)?;
    let result = merge_in_target(source_path, target_branch, head);
    let cleanup = imported.cleanup(source_path);
    finish_with_cleanup(result, cleanup)
}

pub fn push(
    directory: &ReviewDirectory,
    source_path: &Path,
    head: &str,
    merged: bool,
    remote: &str,
    branch: &str,
) -> Result<GitStepOutcome, String> {
    push_typed(directory, source_path, head, merged, remote, branch)
        .map_err(|error| error.to_string())
}

pub fn push_typed(
    directory: &ReviewDirectory,
    source_path: &Path,
    head: &str,
    merged: bool,
    remote: &str,
    branch: &str,
) -> Result<GitStepOutcome, GitActionError> {
    push_with_runner(
        directory,
        source_path,
        head,
        merged,
        remote,
        branch,
        |path, args| git_action(path, args, Duration::from_secs(600)),
    )
}

fn push_with_runner(
    directory: &ReviewDirectory,
    source_path: &Path,
    head: &str,
    merged: bool,
    remote: &str,
    branch: &str,
    run: impl FnOnce(&Path, &[&str]) -> Result<String, GitActionError>,
) -> Result<GitStepOutcome, GitActionError> {
    source_repo(directory, source_path).map_err(GitActionError::Failed)?;
    valid_branch(source_path, branch).map_err(GitActionError::Failed)?;
    valid_remote(source_path, remote).map_err(GitActionError::Failed)?;
    if !merged && head != saved_head(directory).map_err(GitActionError::Failed)? {
        return Err(GitActionError::Failed(
            "push head differs from saved review head".into(),
        ));
    }
    let oid =
        git2::Oid::from_str(head).map_err(|error| GitActionError::Failed(error.to_string()))?;
    let imported = if merged {
        git2::Repository::open(source_path)
            .map_err(|error| GitActionError::Failed(error.to_string()))?
            .find_commit(oid)
            .map_err(|error| GitActionError::Failed(error.to_string()))?;
        PrivateImport(None)
    } else {
        import_if_needed(directory, source_path, head).map_err(GitActionError::Failed)?
    };
    let refspec = format!("{head}:refs/heads/{branch}");
    let result = run(
        source_path,
        &[
            "push",
            "--porcelain",
            "--no-follow-tags",
            "--recurse-submodules=no",
            "--",
            remote,
            &refspec,
        ],
    );
    let result = match result {
        Ok(_) => Ok(GitStepOutcome {
            head: head.into(),
            warning: None,
        }),
        Err(GitActionError::TimedOut(timeout)) => Err(GitActionError::OutcomeUnknown(format!(
            "{timeout}; push outcome unknown for {remote}/{branch}; check the remote before retrying"
        ))),
        Err(error) => Err(error),
    };
    let cleanup = imported.cleanup(source_path);
    match (result, cleanup) {
        (Ok(outcome), cleanup) => {
            finish_with_cleanup(Ok(outcome), cleanup).map_err(GitActionError::Failed)
        }
        (Err(error), Ok(())) => Err(error),
        (Err(GitActionError::OutcomeUnknown(error)), Err(cleanup)) => Err(
            GitActionError::OutcomeUnknown(format!("{error}; cleanup failed: {cleanup}")),
        ),
        (Err(error), Err(cleanup)) => Err(GitActionError::Failed(format!(
            "{error}; cleanup failed: {cleanup}"
        ))),
    }
}

fn source_repo(directory: &ReviewDirectory, source_path: &Path) -> Result<(), String> {
    if !directory.is_git || directory.head.is_none() {
        return Err("review source has no saved Git head".into());
    }
    git2::Repository::open(source_path).map_err(|error| error.to_string())?;
    Ok(())
}

fn valid_remote(repo: &Path, remote: &str) -> Result<(), String> {
    if remote.starts_with('-') || !git2::Remote::is_valid_name(remote) {
        return Err(format!("invalid remote name {remote:?}"));
    }
    if !git(repo, &["remote"])?.lines().any(|name| name == remote) {
        return Err(format!("remote {remote:?} is not configured"));
    }
    let urls = git(
        repo,
        &["remote", "get-url", "--push", "--all", "--", remote],
    )?;
    if urls.trim().is_empty() {
        return Err(format!("remote {remote:?} has no push location"));
    }
    for url in urls.lines() {
        usable_remote_url(url).map_err(|reason| format!("remote {remote:?}: {reason}"))?;
    }
    Ok(())
}

fn saved_head(directory: &ReviewDirectory) -> Result<&str, String> {
    directory
        .head
        .as_deref()
        .ok_or_else(|| "review source has no saved Git head".into())
}

fn valid_branch(repo: &Path, branch: &str) -> Result<(), String> {
    if branch.is_empty() || branch.starts_with('-') {
        return Err("invalid destination branch".into());
    }
    git(repo, &["check-ref-format", &format!("refs/heads/{branch}")]).map(|_| ())
}

fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let args = args.iter().map(OsStr::new).collect::<Vec<_>>();
    git_os(repo, &args)
}

fn git_os(repo: &Path, args: &[&OsStr]) -> Result<String, String> {
    let output = run_git_unattended(repo, args, Duration::from_secs(30))
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(git_failure(args, &output).to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn git_action(repo: &Path, args: &[&str], deadline: Duration) -> Result<String, GitActionError> {
    let os_args = args.iter().map(OsStr::new).collect::<Vec<_>>();
    let output = run_git_unattended(repo, &os_args, deadline).map_err(|error| {
        if error.kind() == ErrorKind::TimedOut {
            GitActionError::TimedOut(error.to_string())
        } else {
            GitActionError::Failed(error.to_string())
        }
    })?;
    if !output.status.success() {
        return Err(GitActionError::Failed(
            git_failure(&os_args, &output).to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn checked_out_at(repo: &Path, target_ref: &str) -> Result<Option<PathBuf>, String> {
    for (path, branch) in worktree_placements(repo)? {
        if branch.as_deref() != Some(target_ref) {
            continue;
        }
        if looks_like_review_target(&path) || is_owned_review_target(repo, &path) {
            return Err(format!(
                "temporary review checkout remains at {}; retry after recovery",
                path.display()
            ));
        }
        return Ok(Some(path));
    }
    Ok(None)
}

fn worktree_placements(repo: &Path) -> Result<Vec<(PathBuf, Option<String>)>, String> {
    let listing = git(repo, &["worktree", "list", "--porcelain", "-z"])?;
    Ok(listing
        .split("\0\0")
        .filter_map(|block| {
            let mut path = None;
            let mut branch = None;
            for line in block.split('\0') {
                if let Some(value) = line.strip_prefix("worktree ") {
                    path = Some(PathBuf::from(value));
                }
                if let Some(value) = line.strip_prefix("branch ") {
                    branch = Some(value);
                }
            }
            path.map(|path| (path, branch.map(str::to_owned)))
        })
        .collect())
}

fn looks_like_review_target(path: &Path) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    path.file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with("review-"))
        && parent
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("build-review-merge-"))
}

fn merge_in_target(source: &Path, branch: &str, head: &str) -> Result<GitStepOutcome, String> {
    merge_in_target_with_before_add(source, branch, head, || {})
}

fn merge_in_target_with_before_add(
    source: &Path,
    branch: &str,
    head: &str,
    before_add: impl FnOnce(),
) -> Result<GitStepOutcome, String> {
    let repo = git2::Repository::open(source).map_err(|error| error.to_string())?;
    if let Some(reason) = unfinished_on_branch(&repo, branch) {
        return Err(reason);
    }
    let target_ref = format!("refs/heads/{branch}");
    if let Some(checkout) = checked_out_at(source, &target_ref)? {
        return merge_checkout(&checkout, branch, head);
    }
    before_add();
    let owner = std::env::temp_dir().join(format!("build-review-merge-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&owner).map_err(|error| error.to_string())?;
    let checkout = owner.join(format!("review-{}", uuid::Uuid::new_v4()));
    match materialize_review_target(source, branch, &checkout) {
        Ok(_) => {}
        Err(error) => {
            let cleanup = std::fs::remove_dir(&owner);
            if checkout.exists() || cleanup.is_err() {
                return Err(format!(
                    "{error}; temporary checkout may remain at {}",
                    checkout.display()
                ));
            }
            if let Some(other) = checked_out_at(source, &target_ref)? {
                return merge_checkout(&other, branch, head);
            }
            return Err(error);
        }
    }
    let result = merge_checkout(&checkout, branch, head);
    let cleanup = remove_review_target(source, branch, &checkout).map_err(|error| {
        format!(
            "temporary checkout {} could not be removed: {error}",
            checkout.display()
        )
    });
    if cleanup.is_ok() {
        let _ = std::fs::remove_dir(&owner);
    }
    finish_with_cleanup(result, cleanup)
}

fn merge_checkout(checkout: &Path, branch: &str, head: &str) -> Result<GitStepOutcome, String> {
    merge_checkout_with_runner(checkout, branch, head, |path, args| {
        merge_git_action(path, args)
    })
}

fn merge_git_action(checkout: &Path, args: &[&str]) -> Result<String, GitActionError> {
    let os_args = args.iter().map(OsStr::new).collect::<Vec<_>>();
    let temporary = is_owned_review_target(checkout, checkout);
    if temporary {
        mark_review_target_launch(checkout).map_err(GitActionError::Failed)?;
    }
    let mut owned_lock = None;
    let mut marker_error = None;
    let mut observer = |event| match event {
        GitProcessEvent::Started(pid) => {
            if temporary {
                if let Err(error) = record_review_target_process(checkout, pid) {
                    marker_error = Some(error);
                    #[cfg(unix)]
                    unsafe {
                        libc::kill(-(pid as i32), libc::SIGKILL);
                    }
                }
            }
        }
        GitProcessEvent::BeforeTimeoutKill(pid) => {
            if let Ok(repo) = git2::Repository::open(checkout) {
                owned_lock = child_holds_index_lock(pid, &repo.path().join("index.lock"));
            }
        }
    };
    let output =
        run_git_unattended_observed(checkout, &os_args, Duration::from_secs(300), &mut observer)
            .map_err(|error| {
                if error.kind() == ErrorKind::TimedOut {
                    match owned_lock {
                        Some(lock) => {
                            GitActionError::TimedOutWithOwnedLock(error.to_string(), lock)
                        }
                        None => GitActionError::TimedOut(error.to_string()),
                    }
                } else {
                    GitActionError::Failed(error.to_string())
                }
            })?;
    if let Some(error) = marker_error {
        return Err(GitActionError::Failed(format!(
            "review merge child identity could not be recorded: {error}"
        )));
    }
    if !output.status.success() {
        return Err(GitActionError::Failed(
            git_failure(&os_args, &output).to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn merge_checkout_with_runner(
    checkout: &Path,
    branch: &str,
    head: &str,
    run: impl FnOnce(&Path, &[&str]) -> Result<String, GitActionError>,
) -> Result<GitStepOutcome, String> {
    let repo = git2::Repository::open(checkout).map_err(|error| error.to_string())?;
    let expected = format!("refs/heads/{branch}");
    if repo
        .head()
        .ok()
        .and_then(|head| head.name().map(str::to_owned))
        != Some(expected)
    {
        return Err(format!(
            "target checkout {} changed branches",
            checkout.display()
        ));
    }
    if repo.state() != git2::RepositoryState::Clean {
        return Err(format!(
            "target checkout {} has a Git operation in progress",
            checkout.display()
        ));
    }
    if !git(
        checkout,
        &["status", "--porcelain", "--untracked-files=all"],
    )?
    .is_empty()
    {
        return Err(format!(
            "target checkout {} must be clean",
            checkout.display()
        ));
    }
    refuse_ignored_overwrite(&repo, checkout, head)?;
    let before = repo
        .head()
        .map_err(|error| error.to_string())?
        .target()
        .ok_or("target has no HEAD commit")?;
    let merged = run(
        checkout,
        &[
            "merge",
            "--no-ff",
            "--commit",
            "--no-squash",
            "--no-gpg-sign",
            "--no-edit",
            "--no-overwrite-ignore",
            head,
        ],
    );
    if let Err(error) = &merged {
        match error {
            GitActionError::TimedOut(message) => {
                return timed_out_merge(&repo, checkout, branch, head, before, message, None)
            }
            GitActionError::TimedOutWithOwnedLock(message, lock) => {
                return timed_out_merge(&repo, checkout, branch, head, before, message, Some(*lock))
            }
            _ => {}
        }
    }
    let merged = merged
        .map_err(|error| error.to_string())
        .and_then(|_| merged_tip(&repo, checkout, branch, head));
    match merged {
        Ok(tip) => Ok(GitStepOutcome {
            head: tip,
            warning: None,
        }),
        Err(error) => {
            if repo.state() == git2::RepositoryState::Merge {
                let abort = git(checkout, &["merge", "--abort"]);
                if let Err(abort_error) = abort {
                    return Err(format!(
                        "{error}; merge abort failed in {}: {abort_error}",
                        checkout.display()
                    ));
                }
            }
            Err(error)
        }
    }
}

fn timed_out_merge(
    repo: &git2::Repository,
    checkout: &Path,
    branch: &str,
    head: &str,
    before: git2::Oid,
    timeout: &str,
    owned_lock: Option<ObservedLock>,
) -> Result<GitStepOutcome, String> {
    let current = repo.head().map_err(|error| error.to_string())?;
    let expected = format!("refs/heads/{branch}");
    if current.name() != Some(expected.as_str()) {
        return Err(format!(
            "{timeout}; target branch changed; manual recovery needed in {}",
            checkout.display()
        ));
    }
    if current.target() != Some(before) {
        return merged_tip(repo, checkout, branch, head)
            .map(|tip| GitStepOutcome {
                head: tip,
                warning: Some(format!(
                    "merge command timed out; resulting tip verified in {}",
                    checkout.display()
                )),
            })
            .map_err(|error| format!("{timeout}; target moved and outcome is unknown: {error}"));
    }
    match restore_timed_out_merge(repo, checkout, branch, head, before, owned_lock) {
        Ok(()) => Err(format!(
            "{timeout}; merge stopped and original checkout restored"
        )),
        Err(error) => Err(format!(
            "{timeout}; manual recovery needed in {}: {error}",
            checkout.display()
        )),
    }
}

fn restore_timed_out_merge(
    repo: &git2::Repository,
    checkout: &Path,
    branch: &str,
    head: &str,
    before: git2::Oid,
    owned_lock: Option<ObservedLock>,
) -> Result<(), String> {
    let lock_path = repo.path().join("index.lock");
    let lock_identity_now = lock_identity(&lock_path);
    if lock_identity_now.is_some() && lock_identity_now != owned_lock {
        return Err(
            "index lock remains without proof it belonged to the timed-out Git child".into(),
        );
    }
    let current = repo
        .find_commit(before)
        .map_err(|error| error.to_string())?
        .tree()
        .map_err(|error| error.to_string())?;
    let incoming = repo
        .find_commit(git2::Oid::from_str(head).map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())?
        .tree()
        .map_err(|error| error.to_string())?;
    let diff = repo
        .diff_tree_to_tree(Some(&current), Some(&incoming), None)
        .map_err(|error| error.to_string())?;
    let touched = diff
        .deltas()
        .filter_map(|delta| delta.new_file().path().or_else(|| delta.old_file().path()))
        .map(Path::to_path_buf)
        .collect::<std::collections::HashSet<_>>();
    let mut options = git2::StatusOptions::new();
    options.include_untracked(true).recurse_untracked_dirs(true);
    let index = repo.index().map_err(|error| error.to_string())?;
    let mut partial_worktree = Vec::new();
    for entry in repo
        .statuses(Some(&mut options))
        .map_err(|error| error.to_string())?
        .iter()
    {
        let path = entry.path().ok_or("changed path cannot be verified")?;
        if !touched.contains(Path::new(path)) {
            return Err(format!(
                "unrelated changed path {path} may belong to another process"
            ));
        }
        let relative = Path::new(path);
        let indexed = index.get_path(relative, 0).map(|entry| entry.id);
        let original = current.get_path(relative).ok().map(|entry| entry.id());
        let selected = incoming.get_path(relative).ok().map(|entry| entry.id());
        if indexed != original && indexed != selected {
            return Err(format!(
                "changed index content for {path} cannot be attributed to the merge"
            ));
        }
        if repo.state() != git2::RepositoryState::Merge
            && entry.status().intersects(
                git2::Status::WT_MODIFIED
                    | git2::Status::WT_DELETED
                    | git2::Status::WT_NEW
                    | git2::Status::WT_RENAMED
                    | git2::Status::WT_TYPECHANGE,
            )
        {
            let bytes = std::fs::read(checkout.join(relative))
                .map_err(|_| format!("changed working file {path} cannot be verified"))?;
            let blob = git2::Oid::hash_object(git2::ObjectType::Blob, &bytes)
                .map_err(|error| error.to_string())?;
            if owned_lock.is_none()
                || indexed != original
                || original.is_none()
                || selected != Some(blob)
            {
                return Err(format!(
                    "changed working file {path} cannot be attributed to the merge"
                ));
            }
            partial_worktree.push(path.to_owned());
        }
    }
    let expected = format!("refs/heads/{branch}");
    if repo
        .head()
        .ok()
        .is_none_or(|head| head.name() != Some(expected.as_str()) || head.target() != Some(before))
    {
        return Err("target head moved before recovery".into());
    }
    if lock_identity_now.is_some() {
        if lock_identity(&lock_path) != owned_lock {
            return Err("index lock changed before recovery".into());
        }
        std::fs::remove_file(&lock_path).map_err(|error| error.to_string())?;
    }
    for path in partial_worktree {
        git(
            checkout,
            &[
                "restore",
                "--source",
                &before.to_string(),
                "--worktree",
                "--",
                &path,
            ],
        )?;
    }
    if repo.state() == git2::RepositoryState::Merge {
        git(checkout, &["merge", "--abort"])?;
    } else {
        git(checkout, &["reset", "--merge", &before.to_string()])?;
    }
    let after = git2::Repository::open(checkout).map_err(|error| error.to_string())?;
    if after.state() != git2::RepositoryState::Clean
        || after.head().ok().and_then(|head| head.target()) != Some(before)
        || !git(
            checkout,
            &["status", "--porcelain", "--untracked-files=all"],
        )?
        .is_empty()
    {
        return Err("checkout could not be verified after restoration".into());
    }
    Ok(())
}

fn merged_tip(
    repo: &git2::Repository,
    checkout: &Path,
    branch: &str,
    head: &str,
) -> Result<String, String> {
    if repo.state() != git2::RepositoryState::Clean {
        return Err(format!("merge in {} did not finish", checkout.display()));
    }
    let current = repo.head().map_err(|error| error.to_string())?;
    let expected = format!("refs/heads/{branch}");
    if current.name() != Some(expected.as_str()) {
        return Err(format!(
            "merge in {} switched target branches",
            checkout.display()
        ));
    }
    let tip = current.target().ok_or("merged target has no commit")?;
    let incoming = git2::Oid::from_str(head).map_err(|error| error.to_string())?;
    if tip != incoming
        && !repo
            .graph_descendant_of(tip, incoming)
            .map_err(|error| error.to_string())?
    {
        return Err(format!(
            "merge in {} did not include the saved head",
            checkout.display()
        ));
    }
    Ok(tip.to_string())
}

fn refuse_ignored_overwrite(
    repo: &git2::Repository,
    checkout: &Path,
    head: &str,
) -> Result<(), String> {
    let current = repo
        .head()
        .map_err(|error| error.to_string())?
        .peel_to_tree()
        .map_err(|error| error.to_string())?;
    let incoming = repo
        .find_commit(git2::Oid::from_str(head).map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())?
        .tree()
        .map_err(|error| error.to_string())?;
    let diff = repo
        .diff_tree_to_tree(Some(&current), Some(&incoming), None)
        .map_err(|error| error.to_string())?;
    for delta in diff.deltas() {
        let Some(path) = delta.new_file().path().or_else(|| delta.old_file().path()) else {
            continue;
        };
        if checkout.join(path).symlink_metadata().is_ok()
            && repo
                .status_should_ignore(path)
                .map_err(|error| error.to_string())?
        {
            return Err(format!(
                "merge would overwrite ignored file {}",
                checkout.join(path).display()
            ));
        }
    }
    Ok(())
}

struct PrivateImport(Option<String>);

fn import_if_needed(
    directory: &ReviewDirectory,
    source: &Path,
    head: &str,
) -> Result<PrivateImport, String> {
    let source_repo = git2::Repository::open(source).map_err(|error| error.to_string())?;
    let saved_common = directory
        .common_git_dir
        .as_deref()
        .ok_or("saved Git directory is unavailable")?
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if source_repo
        .commondir()
        .canonicalize()
        .map_err(|error| error.to_string())?
        == saved_common
    {
        source_repo
            .find_commit(git2::Oid::from_str(head).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
        return Ok(PrivateImport(None));
    }
    let saved_repo =
        git2::Repository::open_bare(&saved_common).map_err(|error| error.to_string())?;
    saved_repo
        .find_commit(git2::Oid::from_str(head).map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())?;
    let unique = uuid::Uuid::new_v4().simple();
    let private_ref = format!("refs/build/review-import/{unique}");
    let refspec = format!("{head}:{private_ref}");
    let fetch = git_os(
        source,
        &[
            OsStr::new("fetch"),
            OsStr::new("--no-tags"),
            OsStr::new("--no-write-fetch-head"),
            OsStr::new("--no-recurse-submodules"),
            OsStr::new("--"),
            saved_common.as_os_str(),
            OsStr::new(&refspec),
        ],
    );
    if let Err(error) = fetch {
        let cleanup = if git(source, &["show-ref", "--verify", &private_ref]).is_ok() {
            git(source, &["update-ref", "-d", &private_ref]).map(|_| ())
        } else {
            Ok(())
        };
        return match cleanup {
            Ok(()) => Err(error),
            Err(cleanup) => Err(format!(
                "{error}; private import cleanup failed for {private_ref}: {cleanup}"
            )),
        };
    }
    Ok(PrivateImport(Some(private_ref)))
}

impl PrivateImport {
    fn cleanup(self, source: &Path) -> Result<(), String> {
        if let Some(reference) = self.0 {
            git(source, &["update-ref", "-d", &reference]).map(|_| ())
        } else {
            Ok(())
        }
    }
}

fn finish_with_cleanup(
    result: Result<GitStepOutcome, String>,
    cleanup: Result<(), String>,
) -> Result<GitStepOutcome, String> {
    match (result, cleanup) {
        (Ok(result), Ok(())) => Ok(result),
        (Ok(mut result), Err(cleanup)) => {
            result.warning = Some(match result.warning {
                Some(previous) => format!("{previous}; {cleanup}"),
                None => cleanup,
            });
            Ok(result)
        }
        (Err(error), Ok(())) => Err(error),
        (Err(error), Err(cleanup)) => Err(format!("{error}; cleanup failed: {cleanup}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_fixture::{configure_repo, git_command, git_in, init_repo};
    use crate::reviews::model::ReviewDirectoryStatus;

    fn oid(repo: &Path, rev: &str) -> String {
        String::from_utf8(
            git_command(repo, &["rev-parse", rev])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_owned()
    }

    #[test]
    fn recovery_removes_owned_temporary_target_and_keeps_branch() {
        let (_temp, repo) = init_repo();
        git_in(&repo, &["branch", "target"]);
        let owner = tempfile::Builder::new()
            .prefix("build-review-merge-")
            .tempdir()
            .unwrap();
        let checkout = owner
            .path()
            .join(format!("review-{}", uuid::Uuid::new_v4()));
        materialize_review_target(&repo, "target", &checkout).unwrap();
        assert_eq!(recover_temporary_worktrees(&repo).unwrap(), 1);
        assert!(!checkout.exists());
        assert!(
            git_command(&repo, &["show-ref", "--verify", "refs/heads/target"])
                .status()
                .unwrap()
                .success()
        );
        assert!(checked_out_at(&repo, "refs/heads/target")
            .unwrap()
            .is_none());
    }

    #[test]
    fn recovery_prunes_owned_registration_after_temp_directory_vanishes() {
        let (_temp, repo) = init_repo();
        git_in(&repo, &["branch", "target"]);
        let owner = tempfile::Builder::new()
            .prefix("build-review-merge-")
            .tempdir()
            .unwrap();
        let checkout = owner
            .path()
            .join(format!("review-{}", uuid::Uuid::new_v4()));
        materialize_review_target(&repo, "target", &checkout).unwrap();
        std::fs::remove_dir_all(&checkout).unwrap();
        assert_eq!(recover_temporary_worktrees(&repo).unwrap(), 1);
        assert!(checked_out_at(&repo, "refs/heads/target")
            .unwrap()
            .is_none());
        assert!(
            git_command(&repo, &["show-ref", "--verify", "refs/heads/target"])
                .status()
                .unwrap()
                .success()
        );
    }

    #[test]
    fn recovery_removes_stale_lock_after_recorded_child_dies() {
        let (_temp, repo) = init_repo();
        git_in(&repo, &["branch", "target"]);
        let owner = tempfile::Builder::new()
            .prefix("build-review-merge-")
            .tempdir()
            .unwrap();
        let checkout = owner
            .path()
            .join(format!("review-{}", uuid::Uuid::new_v4()));
        materialize_review_target(&repo, "target", &checkout).unwrap();
        let admin = crate::isolation::checkout_git_dir(&checkout).unwrap();
        let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap();
        std::fs::write(
            admin.join("build-review-process"),
            format!("{}\n999999\n0\n", boot.trim()),
        )
        .unwrap();
        std::fs::write(admin.join("index.lock"), "stale").unwrap();
        assert_eq!(recover_temporary_worktrees(&repo).unwrap(), 1);
        assert!(checked_out_at(&repo, "refs/heads/target")
            .unwrap()
            .is_none());
    }

    #[test]
    fn recovery_preserves_lock_with_live_recorded_child() {
        let (_temp, repo) = init_repo();
        git_in(&repo, &["branch", "target"]);
        let owner = tempfile::Builder::new()
            .prefix("build-review-merge-")
            .tempdir()
            .unwrap();
        let checkout = owner
            .path()
            .join(format!("review-{}", uuid::Uuid::new_v4()));
        materialize_review_target(&repo, "target", &checkout).unwrap();
        crate::isolation::worktree::record_review_target_process(&checkout, std::process::id())
            .unwrap();
        let admin = crate::isolation::checkout_git_dir(&checkout).unwrap();
        std::fs::write(admin.join("index.lock"), "live").unwrap();
        assert!(recover_temporary_worktrees(&repo)
            .unwrap_err()
            .contains("live merge child"));
        assert!(admin.join("index.lock").exists());
        assert!(checkout.exists());
    }

    #[test]
    fn leftover_review_checkout_is_never_used_as_user_target() {
        let (_temp, repo) = init_repo();
        git_in(&repo, &["branch", "target"]);
        let owner = tempfile::Builder::new()
            .prefix("build-review-merge-")
            .tempdir()
            .unwrap();
        let checkout = owner
            .path()
            .join(format!("review-{}", uuid::Uuid::new_v4()));
        materialize_review_target(&repo, "target", &checkout).unwrap();
        let error = checked_out_at(&repo, "refs/heads/target").unwrap_err();
        assert!(error.contains("temporary review checkout"), "{error}");
    }

    #[test]
    fn cleanup_failure_keeps_successful_git_tip() {
        let outcome = GitStepOutcome {
            head: "abc".into(),
            warning: None,
        };
        let result =
            finish_with_cleanup(Ok(outcome), Err("leftover /tmp/checkout".into())).unwrap();
        assert_eq!(result.head, "abc");
        assert_eq!(result.warning.as_deref(), Some("leftover /tmp/checkout"));
    }

    #[test]
    fn nested_cleanup_failures_keep_both_leftover_paths() {
        let outcome = GitStepOutcome {
            head: "abc".into(),
            warning: None,
        };
        let first = finish_with_cleanup(Ok(outcome), Err("checkout /tmp/a".into()));
        let result = finish_with_cleanup(first, Err("private ref refs/build/b".into())).unwrap();
        assert_eq!(result.head, "abc");
        assert_eq!(
            result.warning.as_deref(),
            Some("checkout /tmp/a; private ref refs/build/b")
        );
    }

    fn directory(repo: &Path, source: &Path) -> ReviewDirectory {
        ReviewDirectory {
            id: "dir".into(),
            source_id: "source".into(),
            name: "repo".into(),
            path: repo.into(),
            source_path: source.into(),
            is_git: true,
            status: ReviewDirectoryStatus::Git,
            reason: None,
            common_git_dir: Some(
                git2::Repository::open(repo)
                    .unwrap()
                    .commondir()
                    .canonicalize()
                    .unwrap(),
            ),
            branch: Some("feature".into()),
            base: None,
            head: Some(oid(repo, "HEAD")),
            uncommitted_files: Some(0),
        }
    }

    fn feature(repo: &Path) -> ReviewDirectory {
        git_in(repo, &["checkout", "-b", "feature"]);
        std::fs::write(repo.join("feature.txt"), "feature\n").unwrap();
        git_in(repo, &["add", "."]);
        git_in(repo, &["commit", "-m", "feature"]);
        let saved = directory(repo, repo);
        git_in(repo, &["checkout", "main"]);
        saved
    }

    #[test]
    fn checked_out_target_merges_saved_head_without_touching_source() {
        let (_temp, repo) = init_repo();
        let saved = feature(&repo);
        let result = merge(&saved, &repo, "main").unwrap();
        assert_eq!(result.head, oid(&repo, "main"));
        assert_eq!(oid(&repo, "main^2"), saved.head.unwrap());
        assert!(repo.join("feature.txt").is_file());
    }

    #[test]
    fn configured_no_commit_cannot_make_success_an_unfinished_merge() {
        let (_temp, repo) = init_repo();
        let saved = feature(&repo);
        git_in(
            &repo,
            &["config", "branch.main.mergeoptions", "--no-commit"],
        );
        let result = merge(&saved, &repo, "main").unwrap();
        assert_eq!(result.head, oid(&repo, "main"));
        assert_eq!(oid(&repo, "main^2"), saved.head.unwrap());
        assert_eq!(
            git2::Repository::open(&repo).unwrap().state(),
            git2::RepositoryState::Clean
        );
    }

    #[test]
    fn configured_squash_cannot_hide_saved_head_from_merge_history() {
        let (_temp, repo) = init_repo();
        let saved = feature(&repo);
        git_in(&repo, &["config", "branch.main.mergeoptions", "--squash"]);
        let result = merge(&saved, &repo, "main").unwrap();
        assert_eq!(result.head, oid(&repo, "main"));
        assert_eq!(oid(&repo, "main^2"), saved.head.unwrap());
        assert_eq!(
            git2::Repository::open(&repo).unwrap().state(),
            git2::RepositoryState::Clean
        );
    }

    #[test]
    fn merge_does_not_invoke_repository_commit_signer() {
        let (_temp, repo) = init_repo();
        let saved = feature(&repo);
        git_in(&repo, &["config", "commit.gpgsign", "true"]);
        git_in(&repo, &["config", "gpg.program", "false"]);
        let result = merge(&saved, &repo, "main").unwrap();
        assert_eq!(result.head, oid(&repo, "main"));
    }

    #[test]
    fn timed_out_merge_restores_partial_tree_without_merge_head() {
        let (_temp, repo) = init_repo();
        let saved = feature(&repo);
        let before = oid(&repo, "main");
        let head = saved.head.as_deref().unwrap();
        let error = merge_checkout_with_runner(&repo, "main", head, |path, _| {
            git_in(path, &["checkout", head, "--", "feature.txt"]);
            Err(GitActionError::TimedOut("merge timed out".into()))
        })
        .unwrap_err();
        assert!(error.contains("restored"), "{error}");
        assert_eq!(oid(&repo, "main"), before);
        assert!(!repo.join("feature.txt").exists());
    }

    #[test]
    fn timed_out_merge_removes_only_observed_child_lock() {
        let (_temp, repo) = init_repo();
        let saved = feature(&repo);
        let head = saved.head.as_deref().unwrap();
        let admin = repo.join(".git");
        let error = merge_checkout_with_runner(&repo, "main", head, |path, _| {
            git_in(path, &["checkout", head, "--", "feature.txt"]);
            std::fs::write(admin.join("index.lock"), "owned").unwrap();
            Err(GitActionError::TimedOutWithOwnedLock(
                "merge timed out".into(),
                lock_identity(&admin.join("index.lock")).unwrap(),
            ))
        })
        .unwrap_err();
        assert!(error.contains("restored"), "{error}");
        assert!(!admin.join("index.lock").exists());
        assert!(!repo.join("feature.txt").exists());
    }

    #[test]
    fn timed_out_merge_restores_verified_partial_worktree_write() {
        let (_temp, repo) = init_repo();
        git_in(&repo, &["checkout", "-b", "feature"]);
        std::fs::write(repo.join("README.md"), "incoming\n").unwrap();
        git_in(&repo, &["commit", "-am", "incoming"]);
        let saved = directory(&repo, &repo);
        git_in(&repo, &["checkout", "main"]);
        let admin = repo.join(".git");
        let error =
            merge_checkout_with_runner(&repo, "main", saved.head.as_deref().unwrap(), |path, _| {
                std::fs::write(path.join("README.md"), "incoming\n").unwrap();
                std::fs::write(admin.join("index.lock"), "pending index").unwrap();
                Err(GitActionError::TimedOutWithOwnedLock(
                    "merge timed out".into(),
                    lock_identity(&admin.join("index.lock")).unwrap(),
                ))
            })
            .unwrap_err();
        assert!(error.contains("restored"), "{error}");
        assert_eq!(
            std::fs::read_to_string(repo.join("README.md")).unwrap(),
            "# project\n"
        );
    }

    #[test]
    fn timed_out_merge_preserves_unrelated_external_staged_edit() {
        let (_temp, repo) = init_repo();
        std::fs::write(repo.join("unrelated.txt"), "before\n").unwrap();
        git_in(&repo, &["add", "unrelated.txt"]);
        git_in(&repo, &["commit", "-m", "unrelated"]);
        let saved = feature(&repo);
        let head = saved.head.as_deref().unwrap();
        let error = merge_checkout_with_runner(&repo, "main", head, |path, _| {
            std::fs::write(path.join("unrelated.txt"), "external\n").unwrap();
            git_in(path, &["add", "unrelated.txt"]);
            Err(GitActionError::TimedOut("merge timed out".into()))
        })
        .unwrap_err();
        assert!(error.contains("manual recovery"), "{error}");
        assert_eq!(
            std::fs::read_to_string(repo.join("unrelated.txt")).unwrap(),
            "external\n"
        );
    }

    #[test]
    fn timed_out_merge_preserves_external_staged_edit_to_touched_file() {
        let (_temp, repo) = init_repo();
        git_in(&repo, &["checkout", "-b", "feature"]);
        std::fs::write(repo.join("README.md"), "incoming\n").unwrap();
        git_in(&repo, &["commit", "-am", "incoming"]);
        let saved = directory(&repo, &repo);
        git_in(&repo, &["checkout", "main"]);
        let error =
            merge_checkout_with_runner(&repo, "main", saved.head.as_deref().unwrap(), |path, _| {
                std::fs::write(path.join("README.md"), "external\n").unwrap();
                git_in(path, &["add", "README.md"]);
                Err(GitActionError::TimedOut("merge timed out".into()))
            })
            .unwrap_err();
        assert!(error.contains("manual recovery"), "{error}");
        assert_eq!(
            std::fs::read_to_string(repo.join("README.md")).unwrap(),
            "external\n"
        );
    }

    #[test]
    fn dirty_checked_out_target_refuses_without_staging() {
        let (_temp, repo) = init_repo();
        let saved = feature(&repo);
        std::fs::write(repo.join("README.md"), "dirty\n").unwrap();
        let before = oid(&repo, "main");
        assert!(merge(&saved, &repo, "main").unwrap_err().contains("clean"));
        assert_eq!(oid(&repo, "main"), before);
        assert_eq!(
            std::fs::read_to_string(repo.join("README.md")).unwrap(),
            "dirty\n"
        );
    }

    #[test]
    fn ignored_target_file_is_not_overwritten_by_merge() {
        let (_temp, repo) = init_repo();
        git_in(&repo, &["checkout", "-b", "feature"]);
        std::fs::write(repo.join("generated.txt"), "committed\n").unwrap();
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-m", "feature"]);
        let saved = directory(&repo, &repo);
        git_in(&repo, &["checkout", "main"]);
        std::fs::write(repo.join(".gitignore"), "generated.txt\n").unwrap();
        git_in(&repo, &["add", ".gitignore"]);
        git_in(&repo, &["commit", "-m", "ignore generated"]);
        std::fs::write(repo.join("generated.txt"), "user data\n").unwrap();
        let before = oid(&repo, "main");
        assert!(merge(&saved, &repo, "main").is_err());
        assert_eq!(oid(&repo, "main"), before);
        assert_eq!(
            std::fs::read_to_string(repo.join("generated.txt")).unwrap(),
            "user data\n"
        );
    }

    #[test]
    fn conflicting_merge_aborts_only_its_own_operation() {
        let (_temp, repo) = init_repo();
        git_in(&repo, &["checkout", "-b", "feature"]);
        std::fs::write(repo.join("README.md"), "feature\n").unwrap();
        git_in(&repo, &["commit", "-am", "feature"]);
        let saved = directory(&repo, &repo);
        git_in(&repo, &["checkout", "main"]);
        std::fs::write(repo.join("README.md"), "main\n").unwrap();
        git_in(&repo, &["commit", "-am", "main"]);
        let before = oid(&repo, "main");
        assert!(merge(&saved, &repo, "main")
            .unwrap_err()
            .contains("CONFLICT"));
        assert_eq!(oid(&repo, "main"), before);
        assert_eq!(
            git2::Repository::open(&repo).unwrap().state(),
            git2::RepositoryState::Clean
        );
        assert_eq!(
            std::fs::read_to_string(repo.join("README.md")).unwrap(),
            "main\n"
        );
    }

    #[test]
    fn conflicting_merge_in_temporary_checkout_cleans_registry() {
        let (_temp, repo) = init_repo();
        git_in(&repo, &["checkout", "-b", "feature"]);
        std::fs::write(repo.join("README.md"), "feature\n").unwrap();
        git_in(&repo, &["commit", "-am", "feature"]);
        let saved = directory(&repo, &repo);
        git_in(&repo, &["checkout", "main"]);
        std::fs::write(repo.join("README.md"), "main\n").unwrap();
        git_in(&repo, &["commit", "-am", "main"]);
        git_in(&repo, &["checkout", "feature"]);
        assert!(merge(&saved, &repo, "main")
            .unwrap_err()
            .contains("CONFLICT"));
        let listed = git_command(&repo, &["worktree", "list", "--porcelain"])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&listed.stdout)
                .matches("worktree ")
                .count(),
            1
        );
        assert_eq!(
            git2::Repository::open(&repo).unwrap().state(),
            git2::RepositoryState::Clean
        );
    }

    #[test]
    fn checked_out_linked_target_is_used_without_checkout_switch() {
        let (temp, repo) = init_repo();
        let saved = feature(&repo);
        let linked = temp.path().join("target");
        git_in(&repo, &["checkout", "feature"]);
        git_in(
            &repo,
            &["worktree", "add", linked.to_str().unwrap(), "main"],
        );
        let outcome = merge(&saved, &repo, "main").unwrap();
        assert_eq!(outcome.head, oid(&linked, "HEAD"));
        assert_eq!(oid(&repo, "HEAD"), saved.head.unwrap());
    }

    #[test]
    fn target_checkout_path_with_newline_is_found_exactly() {
        let (temp, repo) = init_repo();
        let saved = feature(&repo);
        let linked = temp.path().join("target\ncheckout");
        git_in(&repo, &["checkout", "feature"]);
        git_in(
            &repo,
            &["worktree", "add", linked.to_str().unwrap(), "main"],
        );
        assert_eq!(
            merge(&saved, &repo, "main").unwrap().head,
            oid(&linked, "HEAD")
        );
    }

    #[test]
    fn placement_is_rechecked_when_target_becomes_checked_out() {
        let (temp, repo) = init_repo();
        let saved = feature(&repo);
        git_in(&repo, &["checkout", "feature"]);
        let linked = temp.path().join("acquired");
        let result =
            merge_in_target_with_before_add(&repo, "main", saved.head.as_deref().unwrap(), || {
                git_in(
                    &repo,
                    &["worktree", "add", linked.to_str().unwrap(), "main"],
                )
            })
            .unwrap();
        assert_eq!(result.head, oid(&linked, "HEAD"));
    }

    #[test]
    fn detached_bisect_target_is_refused() {
        let (_temp, repo) = init_repo();
        let saved = feature(&repo);
        git_in(&repo, &["checkout", "--detach"]);
        std::fs::write(repo.join(".git/BISECT_START"), "main\n").unwrap();
        let error = merge(&saved, &repo, "main").unwrap_err();
        assert!(error.contains("bisect"), "{error}");
    }

    #[test]
    fn unchecked_target_uses_and_removes_temporary_checkout() {
        let (temp, repo) = init_repo();
        let saved = feature(&repo);
        git_in(&repo, &["checkout", "feature"]);
        let result = merge(&saved, &repo, "main").unwrap();
        assert_eq!(result.head, oid(&repo, "main"));
        assert_eq!(oid(&repo, "HEAD"), saved.head.unwrap());
        assert!(!temp.path().join("review-merge").exists());
        let listed = git_command(&repo, &["worktree", "list", "--porcelain"])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&listed.stdout)
                .matches("worktree ")
                .count(),
            1
        );
    }

    #[test]
    fn separate_clone_imports_saved_commit_privately() {
        let (temp, source) = init_repo();
        let clone = temp.path().join("clone");
        git_in(
            temp.path(),
            &["clone", source.to_str().unwrap(), clone.to_str().unwrap()],
        );
        configure_repo(&clone);
        git_in(&clone, &["checkout", "-b", "feature"]);
        std::fs::write(clone.join("feature.txt"), "clone\n").unwrap();
        git_in(&clone, &["add", "."]);
        git_in(&clone, &["commit", "-m", "clone"]);
        let saved = directory(&clone, &source);
        let result = merge(&saved, &source, "main").unwrap();
        assert_eq!(result.head, oid(&source, "main"));
        assert_eq!(oid(&source, "main^2"), saved.head.unwrap());
        assert!(
            !git_command(&source, &["show-ref", "--verify", "refs/heads/feature"])
                .status()
                .unwrap()
                .success()
        );
    }

    #[test]
    fn unavailable_clone_checkout_uses_saved_common_git_dir() {
        let (temp, source) = init_repo();
        let clone = temp.path().join("clone");
        git_in(
            temp.path(),
            &["clone", source.to_str().unwrap(), clone.to_str().unwrap()],
        );
        configure_repo(&clone);
        git_in(&clone, &["checkout", "-b", "feature"]);
        std::fs::write(clone.join("feature.txt"), "clone\n").unwrap();
        git_in(&clone, &["add", "."]);
        git_in(&clone, &["commit", "-m", "clone"]);
        let mut saved = directory(&clone, &source);
        saved.path = temp.path().join("missing-checkout");
        assert_eq!(
            merge(&saved, &source, "main").unwrap().head,
            oid(&source, "main")
        );
    }

    #[test]
    fn configured_source_may_change_after_snapshot() {
        let (temp, original) = init_repo();
        let replacement = temp.path().join("replacement");
        git_in(
            temp.path(),
            &[
                "clone",
                original.to_str().unwrap(),
                replacement.to_str().unwrap(),
            ],
        );
        configure_repo(&replacement);
        let saved = feature(&original);
        assert_eq!(
            merge(&saved, &replacement, "main").unwrap().head,
            oid(&replacement, "main")
        );
    }

    #[test]
    fn push_refuses_rewritten_transport_helper() {
        let (_temp, repo) = init_repo();
        let saved = directory(&repo, &repo);
        git_in(
            &repo,
            &["remote", "add", "origin", "https://example.com/repo.git"],
        );
        git_in(
            &repo,
            &[
                "config",
                "url.ext::sh -c false.insteadOf",
                "https://example.com/",
            ],
        );
        let error = push(
            &saved,
            &repo,
            saved.head.as_deref().unwrap(),
            false,
            "origin",
            "main",
        )
        .unwrap_err();
        assert!(error.contains("spaces or control characters"), "{error}");
    }

    #[test]
    fn push_is_non_forced_and_reports_rejection() {
        let (temp, repo) = init_repo();
        let bare = temp.path().join("remote.git");
        git_in(temp.path(), &["init", "--bare", bare.to_str().unwrap()]);
        git_in(&repo, &["remote", "add", "origin", bare.to_str().unwrap()]);
        git_in(&repo, &["push", "origin", "main"]);
        let saved = feature(&repo);
        let result = push(
            &saved,
            &repo,
            saved.head.as_deref().unwrap(),
            false,
            "origin",
            "feature",
        )
        .unwrap();
        assert_eq!(result.head, saved.head.as_ref().unwrap().as_str());
        assert_eq!(oid(&bare, "refs/heads/feature"), result.head);
        let other = temp.path().join("other");
        git_in(
            temp.path(),
            &["clone", bare.to_str().unwrap(), other.to_str().unwrap()],
        );
        configure_repo(&other);
        git_in(&other, &["checkout", "feature"]);
        std::fs::write(other.join("later.txt"), "later\n").unwrap();
        git_in(&other, &["add", "."]);
        git_in(&other, &["commit", "-m", "later"]);
        git_in(&other, &["push", "origin", "feature"]);
        let err = push(
            &saved,
            &repo,
            saved.head.as_deref().unwrap(),
            false,
            "origin",
            "feature",
        )
        .unwrap_err();
        assert!(
            err.contains("non-fast-forward") || err.contains("fetch first"),
            "{err}"
        );
    }

    #[test]
    fn push_does_not_follow_tags_from_repository_configuration() {
        let (temp, repo) = init_repo();
        let bare = temp.path().join("remote.git");
        git_in(temp.path(), &["init", "--bare", bare.to_str().unwrap()]);
        git_in(&repo, &["remote", "add", "origin", bare.to_str().unwrap()]);
        let saved = feature(&repo);
        git_in(
            &repo,
            &[
                "tag",
                "-a",
                "release",
                "-m",
                "release",
                saved.head.as_deref().unwrap(),
            ],
        );
        git_in(&repo, &["config", "push.followTags", "true"]);
        push(
            &saved,
            &repo,
            saved.head.as_deref().unwrap(),
            false,
            "origin",
            "feature",
        )
        .unwrap();
        assert!(
            !git_command(&bare, &["show-ref", "--verify", "refs/tags/release"])
                .status()
                .unwrap()
                .success()
        );
    }

    #[test]
    fn timed_out_push_is_unknown_even_if_remote_was_updated() {
        let (temp, repo) = init_repo();
        let bare = temp.path().join("remote.git");
        git_in(temp.path(), &["init", "--bare", bare.to_str().unwrap()]);
        git_in(&repo, &["remote", "add", "origin", bare.to_str().unwrap()]);
        let saved = feature(&repo);
        let head = saved.head.as_deref().unwrap();
        let error = push_with_runner(
            &saved,
            &repo,
            head,
            false,
            "origin",
            "feature",
            |path, _| {
                git_in(
                    path,
                    &["push", "origin", &format!("{head}:refs/heads/feature")],
                );
                Err(GitActionError::TimedOut("push timed out".into()))
            },
        )
        .unwrap_err();
        assert!(
            matches!(error, GitActionError::OutcomeUnknown(_)),
            "{error}"
        );
    }

    #[test]
    fn timed_out_push_with_unmatched_remote_is_outcome_unknown() {
        let (temp, repo) = init_repo();
        let bare = temp.path().join("remote.git");
        git_in(temp.path(), &["init", "--bare", bare.to_str().unwrap()]);
        git_in(&repo, &["remote", "add", "origin", bare.to_str().unwrap()]);
        let saved = feature(&repo);
        let error = push_with_runner(
            &saved,
            &repo,
            saved.head.as_deref().unwrap(),
            false,
            "origin",
            "feature",
            |_, _| Err(GitActionError::TimedOut("push timed out".into())),
        )
        .unwrap_err();
        assert!(
            matches!(error, GitActionError::OutcomeUnknown(_)),
            "{error}"
        );
    }

    #[test]
    fn timed_out_push_checks_pushurl_not_fetch_url() {
        let (temp, repo) = init_repo();
        let fetch = temp.path().join("fetch.git");
        let destination = temp.path().join("push.git");
        git_in(temp.path(), &["init", "--bare", fetch.to_str().unwrap()]);
        git_in(
            temp.path(),
            &["init", "--bare", destination.to_str().unwrap()],
        );
        git_in(&repo, &["remote", "add", "origin", fetch.to_str().unwrap()]);
        git_in(
            &repo,
            &[
                "remote",
                "set-url",
                "--push",
                "origin",
                destination.to_str().unwrap(),
            ],
        );
        let saved = feature(&repo);
        git_in(&repo, &["push", fetch.to_str().unwrap(), "feature"]);
        let error = push_with_runner(
            &saved,
            &repo,
            saved.head.as_deref().unwrap(),
            false,
            "origin",
            "feature",
            |_, _| Err(GitActionError::TimedOut("push timed out".into())),
        )
        .unwrap_err();
        assert!(
            matches!(error, GitActionError::OutcomeUnknown(_)),
            "{error}"
        );
    }
}
