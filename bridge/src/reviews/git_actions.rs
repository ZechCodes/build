//! Explicit Git steps for a saved review source.

use super::actions::ReviewRemote;
use super::model::ReviewDirectory;
use crate::git_process::{git_failure, run_git_unattended};
use crate::isolation::worktree::{materialize_review_target, remove_review_target};
use crate::remote_url::usable_remote_url;
use crate::source_sync::unfinished_on_branch;
use serde::Serialize;
use std::ffi::OsStr;
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
    source_repo(directory, source_path)?;
    valid_branch(source_path, branch)?;
    valid_remote(source_path, remote)?;
    if !merged && head != saved_head(directory)? {
        return Err("push head differs from saved review head".into());
    }
    let oid = git2::Oid::from_str(head).map_err(|error| error.to_string())?;
    let imported = if merged {
        git2::Repository::open(source_path)
            .map_err(|error| error.to_string())?
            .find_commit(oid)
            .map_err(|error| error.to_string())?;
        PrivateImport(None)
    } else {
        import_if_needed(directory, source_path, head)?
    };
    let refspec = format!("{head}:refs/heads/{branch}");
    let result = git(
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
    )
    .map(|_| GitStepOutcome {
        head: head.into(),
        warning: None,
    });
    finish_with_cleanup(result, imported.cleanup(source_path))
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

fn checked_out_at(repo: &Path, target_ref: &str) -> Result<Option<PathBuf>, String> {
    let listing = git(repo, &["worktree", "list", "--porcelain", "-z"])?;
    Ok(listing.split("\0\0").find_map(|block| {
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
        (branch == Some(target_ref)).then_some(path).flatten()
    }))
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
    let merged = git(
        checkout,
        &[
            "merge",
            "--no-ff",
            "--commit",
            "--no-squash",
            "--no-edit",
            "--no-overwrite-ignore",
            head,
        ],
    )
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
}
