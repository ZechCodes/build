//! The copy-on-write clone backend: a checkout made by cloning the whole
//! project directory, `.git` included, with the filesystem's own reflink so it
//! costs no disk and starts warm. [`clone_tree`] is the one platform call —
//! `clonefile` on macOS, a `FICLONE` walk on Linux — and the probe (§4.3) runs
//! it on a single file, so cloning is written in exactly one place.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use super::{
    cow_marker_names, local_branch_ref, write_cow_marker, Isolation, IsolationBackend,
    WorktreeError,
};
use crate::git_process::{run_git, run_git_with_deadline};

#[cfg(target_os = "macos")]
use std::ffi::CString;
#[cfg(target_os = "macos")]
use std::os::unix::ffi::OsStrExt;
#[cfg(target_os = "linux")]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
#[cfg(target_os = "linux")]
use std::os::unix::io::AsRawFd;

/// Clone the file or directory tree at `src` to `dst`, which must not exist.
/// On macOS one `clonefile` clones a whole tree atomically. On Linux the tree
/// is walked: directories and symlinks are recreated and every regular file is
/// reflinked with `FICLONE`, and any other file type is an error. Anywhere
/// else there is no clone at all. On any failure `dst` is removed before the
/// error returns, so a failed clone leaves nothing behind.
#[cfg(target_os = "macos")]
pub(crate) fn clone_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    let source = to_c_path(src)?;
    let destination = to_c_path(dst)?;
    let cloned = unsafe { libc::clonefile(source.as_ptr(), destination.as_ptr(), 0) };
    if cloned != 0 {
        let error = std::io::Error::last_os_error();
        discard(dst);
        return Err(error);
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn to_c_path(path: &Path) -> std::io::Result<CString> {
    CString::new(path.as_os_str().as_bytes())
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))
}

#[cfg(target_os = "linux")]
pub(crate) fn clone_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    clone_walk(src, dst).map_err(|error| {
        discard(dst);
        error
    })
}

#[cfg(target_os = "linux")]
fn clone_walk(src: &Path, dst: &Path) -> std::io::Result<()> {
    let source = std::fs::symlink_metadata(src)?;
    let file_type = source.file_type();
    if file_type.is_dir() {
        std::fs::create_dir(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            clone_walk(&entry.path(), &dst.join(entry.file_name()))?;
        }
        std::fs::set_permissions(dst, source.permissions())?;
        Ok(())
    } else if file_type.is_symlink() {
        std::os::unix::fs::symlink(std::fs::read_link(src)?, dst)
    } else if file_type.is_file() {
        reflink_file(src, dst, source.permissions().mode())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            format!(
                "cannot clone {}: not a regular file, directory or symlink",
                src.display()
            ),
        ))
    }
}

#[cfg(target_os = "linux")]
fn reflink_file(src: &Path, dst: &Path, mode: u32) -> std::io::Result<()> {
    let source = std::fs::File::open(src)?;
    let destination = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(dst)?;
    let cloned = unsafe { libc::ioctl(destination.as_raw_fd(), libc::FICLONE, source.as_raw_fd()) };
    if cloned != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(crate) fn clone_tree(_src: &Path, _dst: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "copy-on-write cloning is only available on macOS and Linux",
    ))
}

/// Materializes a checkout as a copy-on-write clone of the whole project
/// directory, `.git` included, so it starts with the project's build caches
/// already in place and keeps its own git repository. Everything a backend
/// does is whole here; the marker (§4.6) is the only trace that tells the clone
/// apart from any other repository, and this file never spells its name.
#[derive(Clone, Debug)]
pub struct CowBackend;

impl IsolationBackend for CowBackend {
    fn kind(&self) -> Isolation {
        Isolation::Cow
    }

    fn materialize(&self, project: &Path, branch: &str, path: &Path) -> Result<(), WorktreeError> {
        refuse_if_mid_operation(project)?;
        clone_tree(project, path)?;
        populate_clone(project, branch, path).inspect_err(|_| {
            let _ = std::fs::remove_dir_all(path);
        })
    }

    fn verify(&self, project: &Path, path: &Path, branch: &str) -> Result<(), WorktreeError> {
        if !cow_marker_names(path, project) {
            return Err(WorktreeError::Refused(format!(
                "{} is not a copy-on-write clone of this project",
                path.display()
            )));
        }
        let head = run_git(path, &["symbolic-ref", "--short", "HEAD"])?;
        if head.trim() != branch {
            return Err(WorktreeError::Refused(format!(
                "the clone is on {} not {branch}",
                head.trim()
            )));
        }
        Ok(())
    }

    fn publish(&self, project: &Path, path: &Path, branch: &str) -> Result<(), WorktreeError> {
        fetch_branch(project, path, branch)
    }

    fn sync_base(
        &self,
        project: &Path,
        path: &Path,
        base_branch: &str,
    ) -> Result<(), WorktreeError> {
        fetch_branch(path, project, base_branch)
    }

    fn remove(&self, _project: &Path, path: &Path) -> Result<(), WorktreeError> {
        if path.exists() {
            std::fs::remove_dir_all(path)?;
        }
        Ok(())
    }

    fn discover(
        &self,
        project: &Path,
        worktrees_root: &Path,
    ) -> Result<Vec<PathBuf>, WorktreeError> {
        let entries = match std::fs::read_dir(worktrees_root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut clones = Vec::new();
        for entry in entries {
            let path = entry?.path();
            if Isolation::of(&path) == Some(Isolation::Cow) && cow_marker_names(&path, project) {
                clones.push(std::fs::canonicalize(&path)?);
            }
        }
        Ok(clones)
    }

    fn prune(&self, _project: &Path) -> Result<(), WorktreeError> {
        Ok(())
    }

    fn holds_record(&self, _project: &Path, _name: &str) -> Result<bool, WorktreeError> {
        Ok(false)
    }
}

/// Copy the tip of `branch` from the repository at `source` to the one at
/// `into`, by path — a clone has no configured remote, so both `publish` (the
/// clone's tip into the project) and `sync_base` (the project's base into the
/// clone) are one bounded fetch in opposite directions. git refuses to move a
/// branch the destination has checked out, and that refusal is the error.
fn fetch_branch(into: &Path, source: &Path, branch: &str) -> Result<(), WorktreeError> {
    let branch_ref = local_branch_ref(branch);
    let refspec = format!("+{branch_ref}:{branch_ref}");
    let args = [
        OsStr::new("fetch"),
        OsStr::new("--no-tags"),
        OsStr::new("--quiet"),
        OsStr::new("--"),
        source.as_os_str(),
        OsStr::new(&refspec),
    ];
    let output = run_git_with_deadline(into, &args)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail: Vec<&str> = [stderr.trim(), stdout.trim()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect();
        return Err(WorktreeError::Command(format!(
            "git fetch {} {branch}: {}",
            source.display(),
            detail.join("\n")
        )));
    }
    Ok(())
}

/// Refuse to clone a project whose own checkout is mid-operation: a clone of a
/// half-finished merge or rebase would carry the conflict and the state files,
/// and an `index.lock` names a concurrent git the clone would race.
fn refuse_if_mid_operation(project: &Path) -> Result<(), WorktreeError> {
    let repo = git2::Repository::open(project)?;
    if repo.state() != git2::RepositoryState::Clean
        || project.join(".git").join("index.lock").exists()
    {
        return Err(WorktreeError::Refused(
            "the project checkout is mid-operation; finish or abort it first".to_string(),
        ));
    }
    Ok(())
}

/// Steps 3–5 of §4.5, run in the fresh clone at `path`: shed the project's
/// inherited worktree records and lock, mark the clone as Build's, put HEAD on
/// `branch` with a clean tree (ignored directories kept — the warm start), and
/// prove HEAD landed on the project's tip of that branch.
fn populate_clone(project: &Path, branch: &str, path: &Path) -> Result<(), WorktreeError> {
    let git_dir = path.join(".git");
    let inherited_records = git_dir.join("worktrees");
    if inherited_records.exists() {
        std::fs::remove_dir_all(&inherited_records)?;
    }
    let index_lock = git_dir.join("index.lock");
    if index_lock.exists() {
        std::fs::remove_file(&index_lock)?;
    }
    write_cow_marker(path, project)?;

    let head_ref = local_branch_ref(branch);
    run_git(path, &["symbolic-ref", "HEAD", &head_ref])?;
    run_git(path, &["reset", "--hard"])?;
    run_git(path, &["clean", "-fd"])?;

    verify_head_on_tip(project, branch, path)
}

/// The clone's HEAD must be the commit the project's `refs/heads/<branch>`
/// points at, or the warm start did not land where it was told to.
fn verify_head_on_tip(project: &Path, branch: &str, path: &Path) -> Result<(), WorktreeError> {
    let clone = git2::Repository::open(path)?;
    let clone_head = clone.head()?.peel_to_commit()?.id();
    let repo = git2::Repository::open(project)?;
    let tip = repo
        .find_reference(&local_branch_ref(branch))?
        .peel_to_commit()?
        .id();
    if clone_head != tip {
        return Err(WorktreeError::Refused(format!(
            "clone HEAD {clone_head} is not the {branch} tip {tip}"
        )));
    }
    Ok(())
}

/// Remove whatever a failed clone left at `dst` — a partial tree or a single
/// file — so `clone_tree` never leaves a half-made destination behind.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn discard(dst: &Path) {
    if std::fs::remove_dir_all(dst).is_err() {
        let _ = std::fs::remove_file(dst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::isolation::probe::cow_availability;
    #[cfg(target_os = "linux")]
    use std::os::unix::ffi::OsStrExt;

    /// Whether this volume can clone. On failure it prints the reason and
    /// returns false, so a clone test says aloud why it did nothing rather than
    /// passing without exercising anything. Every clone test opens with it.
    fn cow_or_skip(dir: &Path) -> bool {
        let project = dir.join("probe-project");
        std::fs::create_dir_all(project.join(".git")).unwrap();
        let worktrees_root = dir.join("probe-worktrees");
        match cow_availability(&project, &worktrees_root) {
            Ok(()) => true,
            Err(reason) => {
                eprintln!("skipping: {reason}");
                false
            }
        }
    }

    #[test]
    fn clone_tree_reproduces_a_symlink_and_a_subdirectory() {
        let dir = tempfile::tempdir().unwrap();
        if !cow_or_skip(dir.path()) {
            return;
        }
        let src = dir.path().join("src");
        std::fs::create_dir(&src).unwrap();
        std::fs::create_dir(src.join("subdir")).unwrap();
        std::fs::write(src.join("subdir").join("file.txt"), b"warm").unwrap();
        std::os::unix::fs::symlink("subdir/file.txt", src.join("link")).unwrap();

        let dst = dir.path().join("dst");
        clone_tree(&src, &dst).unwrap();

        assert!(dst.join("subdir").is_dir());
        assert_eq!(
            std::fs::read(dst.join("subdir").join("file.txt")).unwrap(),
            b"warm"
        );
        assert_eq!(
            std::fs::read_link(dst.join("link")).unwrap(),
            Path::new("subdir/file.txt")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn clone_tree_errors_on_a_fifo_and_leaves_no_destination() {
        let dir = tempfile::tempdir().unwrap();
        if !cow_or_skip(dir.path()) {
            return;
        }
        let src = dir.path().join("src");
        std::fs::create_dir(&src).unwrap();
        make_fifo(&src.join("pipe"));

        let dst = dir.path().join("dst");
        let error = clone_tree(&src, &dst).unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::Unsupported, "{error}");
        assert!(!dst.exists(), "a failed clone left a destination behind");
    }

    #[cfg(target_os = "linux")]
    fn make_fifo(path: &Path) {
        let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        let made = unsafe { libc::mkfifo(name.as_ptr(), 0o644) };
        assert_eq!(made, 0, "mkfifo failed");
    }

    use crate::git_fixture::{git_in, init_repo, init_repo_named};

    /// The commit `<rev>` names in the repository at `dir`.
    fn rev(dir: &Path, spec: &str) -> String {
        String::from_utf8(
            std::process::Command::new("git")
                .args(["rev-parse", spec])
                .current_dir(dir)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string()
    }

    /// Whether `git <args>` in `dir` exits zero, its failure swallowed — for the
    /// steps a test drives that are allowed to fail (a conflicting merge, a
    /// checkout the clone should permit).
    fn git_ok(dir: &Path, args: &[&str]) -> bool {
        std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap()
            .status
            .success()
    }

    #[test]
    fn materialize_starts_warm_on_the_branch_tip_with_no_worktree_records() {
        let (dir, project) = init_repo();
        if !cow_or_skip(dir.path()) {
            return;
        }
        std::fs::write(project.join(".gitignore"), "build-cache/\n").unwrap();
        git_in(&project, &["add", ".gitignore"]);
        git_in(&project, &["commit", "-m", "ignore build cache"]);
        std::fs::create_dir(project.join("build-cache")).unwrap();
        std::fs::write(project.join("build-cache").join("warm"), b"cached").unwrap();
        git_in(&project, &["branch", "feature"]);

        let worktrees_root = dir.path().join("worktrees");
        std::fs::create_dir_all(&worktrees_root).unwrap();
        let clone = worktrees_root.join("csv-export");
        CowBackend.materialize(&project, "feature", &clone).unwrap();

        assert_eq!(Isolation::of(&clone), Some(Isolation::Cow));
        assert_eq!(
            std::fs::read(clone.join("build-cache").join("warm")).unwrap(),
            b"cached",
            "the ignored build cache did not survive the warm start"
        );
        assert!(
            !clone.join(".git").join("worktrees").exists(),
            "the clone inherited the project's linked-worktree records"
        );
        assert_eq!(
            rev(&clone, "HEAD"),
            rev(&project, "refs/heads/feature"),
            "the clone is not on the branch's tip"
        );
        assert!(
            git_ok(&clone, &["status", "--porcelain"]),
            "git status failed in the clone"
        );
        let dirty = std::process::Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&clone)
            .output()
            .unwrap()
            .stdout;
        assert!(dirty.is_empty(), "the clone is not clean: {dirty:?}");
    }

    #[test]
    fn materialize_refuses_a_project_mid_merge_and_leaves_nothing() {
        let (dir, project) = init_repo();
        if !cow_or_skip(dir.path()) {
            return;
        }
        git_in(&project, &["checkout", "-b", "other"]);
        std::fs::write(project.join("README.md"), "# other\n").unwrap();
        git_in(&project, &["commit", "-am", "other"]);
        git_in(&project, &["checkout", "main"]);
        std::fs::write(project.join("README.md"), "# main\n").unwrap();
        git_in(&project, &["commit", "-am", "main"]);
        assert!(
            !git_ok(&project, &["merge", "other"]),
            "the merge did not conflict, so the project is not mid-operation"
        );
        git_in(&project, &["branch", "feature"]);

        let worktrees_root = dir.path().join("worktrees");
        std::fs::create_dir_all(&worktrees_root).unwrap();
        let clone = worktrees_root.join("csv-export");
        let refused = CowBackend.materialize(&project, "feature", &clone);

        assert!(
            matches!(refused, Err(WorktreeError::Refused(_))),
            "expected a refusal, got {refused:?}"
        );
        assert!(!clone.exists(), "the refusal left something at the path");
    }

    #[test]
    fn materialize_frees_a_branch_a_linked_worktree_of_the_project_holds() {
        let (dir, project) = init_repo();
        if !cow_or_skip(dir.path()) {
            return;
        }
        let linked = dir.path().join("linked");
        git_in(
            &project,
            &[
                "worktree",
                "add",
                linked.to_str().unwrap(),
                "-b",
                "sidebranch",
            ],
        );
        git_in(&project, &["branch", "feature"]);

        let worktrees_root = dir.path().join("worktrees");
        std::fs::create_dir_all(&worktrees_root).unwrap();
        let clone = worktrees_root.join("csv-export");
        CowBackend.materialize(&project, "feature", &clone).unwrap();

        assert!(
            git_ok(&clone, &["checkout", "sidebranch"]),
            "the clone still thinks the linked worktree holds sidebranch"
        );
    }

    /// A materialized clone of `project` on `branch` under a fresh worktrees
    /// root, the shared opening every primitive test below wants.
    fn clone_on(dir: &Path, project: &Path, branch: &str, name: &str) -> PathBuf {
        let worktrees_root = dir.join("worktrees");
        std::fs::create_dir_all(&worktrees_root).unwrap();
        let clone = worktrees_root.join(name);
        CowBackend.materialize(project, branch, &clone).unwrap();
        clone
    }

    #[test]
    fn verify_accepts_the_clone_and_rejects_a_stranger_or_wrong_branch() {
        let (dir, project) = init_repo();
        if !cow_or_skip(dir.path()) {
            return;
        }
        git_in(&project, &["branch", "feature"]);
        let clone = clone_on(dir.path(), &project, "feature", "csv-export");

        CowBackend.verify(&project, &clone, "feature").unwrap();

        assert!(
            CowBackend.verify(&project, &clone, "main").is_err(),
            "verify accepted a clone that is not on the named branch"
        );

        let other = init_repo_named(dir.path(), "other");
        assert!(
            CowBackend.verify(&other, &clone, "feature").is_err(),
            "verify accepted a clone whose marker names another project"
        );
    }

    #[test]
    fn publish_moves_the_project_ref_and_refuses_the_checked_out_branch() {
        let (dir, project) = init_repo();
        if !cow_or_skip(dir.path()) {
            return;
        }
        git_in(&project, &["branch", "feature"]);
        let clone = clone_on(dir.path(), &project, "feature", "csv-export");

        std::fs::write(clone.join("work.txt"), b"work").unwrap();
        git_in(&clone, &["add", "work.txt"]);
        git_in(&clone, &["commit", "-m", "work in the clone"]);

        CowBackend.publish(&project, &clone, "feature").unwrap();
        assert_eq!(
            rev(&project, "refs/heads/feature"),
            rev(&clone, "HEAD"),
            "publish did not move the project's feature ref to the clone's tip"
        );

        // Advance the clone's copy of main so publishing it would move the
        // project's checked-out branch, which git refuses.
        git_in(&clone, &["branch", "-f", "main", "feature"]);
        let refused = CowBackend.publish(&project, &clone, "main");
        assert!(
            matches!(refused, Err(WorktreeError::Command(_))),
            "publishing the project's checked-out branch should fail, got {refused:?}"
        );
    }

    #[test]
    fn sync_base_moves_the_clone_base_ref_and_stat_against_base_sees_it() {
        let (dir, project) = init_repo();
        if !cow_or_skip(dir.path()) {
            return;
        }
        git_in(&project, &["branch", "feature"]);
        let clone = clone_on(dir.path(), &project, "feature", "csv-export");

        std::fs::write(project.join("README.md"), "# moved base\n").unwrap();
        git_in(&project, &["commit", "-am", "advance main"]);

        assert_eq!(
            crate::diff::stat_against_base(&clone, "main")
                .unwrap()
                .files_changed,
            0,
            "the clone already differs from its stale base"
        );

        CowBackend.sync_base(&project, &clone, "main").unwrap();

        assert_eq!(
            rev(&clone, "refs/heads/main"),
            rev(&project, "refs/heads/main"),
            "sync_base did not move the clone's main ref to the project's tip"
        );
        assert!(
            crate::diff::stat_against_base(&clone, "main")
                .unwrap()
                .files_changed
                > 0,
            "stat_against_base does not see the moved base"
        );
    }

    /// `remove` deletes the directory and a repeat on the gone path succeeds
    /// (absence is done). The branch is untouched: branch deletion is the
    /// façade's `keep_branch` policy, not the backend's, so a clone's `remove`
    /// always leaves the branch — the "branch survives" half of the stage
    /// doc's remove test. The "branch gone" half lives with the façade.
    #[test]
    fn remove_deletes_the_clone_and_leaves_the_branch_and_a_repeat_is_success() {
        let (dir, project) = init_repo();
        if !cow_or_skip(dir.path()) {
            return;
        }
        git_in(&project, &["branch", "feature"]);
        let clone = clone_on(dir.path(), &project, "feature", "csv-export");

        CowBackend.remove(&project, &clone).unwrap();
        assert!(!clone.exists(), "remove left the clone directory behind");
        assert!(
            git_ok(&project, &["show-ref", "--verify", "refs/heads/feature"]),
            "remove deleted the branch, which is the façade's job, not the backend's"
        );

        CowBackend
            .remove(&project, &clone)
            .expect("removing an already-gone clone is success");
    }

    #[test]
    fn discover_finds_this_projects_clones_and_skips_others() {
        let (dir, project) = init_repo();
        if !cow_or_skip(dir.path()) {
            return;
        }
        git_in(&project, &["branch", "one"]);
        git_in(&project, &["branch", "two"]);
        let first = clone_on(dir.path(), &project, "one", "first");
        let second = clone_on(dir.path(), &project, "two", "second");
        let worktrees_root = dir.path().join("worktrees");

        // A linked worktree in the same root is not a clone.
        let linked = worktrees_root.join("linked");
        git_in(
            &project,
            &["worktree", "add", linked.to_str().unwrap(), "-b", "side"],
        );

        // A clone of a different project living in the same root.
        let other = init_repo_named(dir.path(), "other");
        git_in(&other, &["branch", "elsewhere"]);
        let other_clone = worktrees_root.join("other-clone");
        CowBackend
            .materialize(&other, "elsewhere", &other_clone)
            .unwrap();

        let mut found = CowBackend.discover(&project, &worktrees_root).unwrap();
        found.sort();
        let mut expected = vec![
            std::fs::canonicalize(&first).unwrap(),
            std::fs::canonicalize(&second).unwrap(),
        ];
        expected.sort();
        assert_eq!(
            found, expected,
            "discover did not find exactly this project's clones"
        );
    }

    #[test]
    fn a_clone_keeps_no_record_and_has_nothing_to_prune() {
        let (_dir, project) = init_repo();
        assert!(
            !CowBackend.holds_record(&project, "csv-export").unwrap(),
            "a clone leaves no registry record"
        );
        CowBackend
            .prune(&project)
            .expect("a clone has no stale records to prune");
    }
}
