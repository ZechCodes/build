//! Reading one repository's Git state in a process of its own, so the reading
//! is bounded.
//!
//! `work_summary` (the test Done uses) runs a status, a history walk and a
//! diff inside libgit2, and none of them can be interrupted: on a large or
//! slow repository it runs as long as it runs. So the service never calls it
//! in the daemon. It starts the bridge again as `build-bridge measure-git`,
//! which reads one repository and prints what it found, and waits on it only
//! while the budget lasts. When the deadline passes or the daemon stops, the
//! reading is killed and reaped before the service goes on, and what it would
//! have said is not known: the workspace is held as `unmeasured`.

use super::artifacts;
use super::budget::{Budget, Unfinished};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::io::{self, Read};
#[cfg(unix)]
use std::os::fd::{AsRawFd, RawFd};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

#[cfg(test)]
mod final_check_tests;

/// The repository a reading is of. An environment variable rather than an
/// argument, so the test binary can stand in for the bridge (its arguments
/// are its test harness's).
pub const REPOSITORY_VAR: &str = "BUILD_GIT_READING_REPOSITORY";
const CANDIDATES_VAR: &str = "BUILD_GIT_READING_CANDIDATES";
#[cfg(test)]
const PAUSE_CANDIDATE_VAR: &str = "BUILD_GIT_READING_PAUSE_CANDIDATE";
/// Keep one environment value well below the per-string exec limit. A larger
/// request is unfinished, so no directory can be moved on an incomplete look.
const MAX_CANDIDATE_REQUEST_BYTES: usize = 64 * 1024;
const MAX_READING_BYTES: usize = 1024 * 1024;

/// How the reading's one line of output begins.
const MARKER: &str = "BUILD-GIT-READING ";

/// How often a running reading is looked at.
const POLL: Duration = Duration::from_millis(5);

/// What one repository's Git state says, as the reading prints it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitReading {
    /// The repository could be read. When it could not, it is held as
    /// `unknown`, the way Done holds it.
    pub read: bool,
    pub pushes: u64,
    pub behind: u64,
    pub dirty: bool,
    pub dirty_files: u64,
    pub newest_commit_ms: Option<i64>,
    /// Every candidate was still ignored and absent from a fresh index.
    pub candidates_valid: bool,
    /// Metadata of that index, for a cheap final change check under the mutex.
    pub index_snapshot: Option<IndexSnapshot>,
    /// Ignore and config inputs read for candidate classification, also
    /// checked by metadata under the mutex.
    pub ignore_snapshots: Vec<IndexSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// A file input to candidate validation. Missing files are remembered too,
/// so creating a new ignore rule after the child exits fails the final check.
pub struct IndexSnapshot {
    path: PathBuf,
    fingerprint: Option<IndexFingerprint>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct IndexFingerprint {
    len: u64,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    modified_seconds: i64,
    #[cfg(unix)]
    modified_nanoseconds: i64,
    #[cfg(unix)]
    changed_seconds: i64,
    #[cfg(unix)]
    changed_nanoseconds: i64,
    #[cfg(not(unix))]
    modified_nanoseconds: Option<u128>,
}

impl IndexSnapshot {
    fn capture(path: PathBuf) -> Option<Self> {
        let fingerprint = match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => return None,
            Ok(metadata) => Some(IndexFingerprint::from(metadata)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(_) => return None,
        };
        Some(Self { path, fingerprint })
    }

    /// A cheap check that the index read by the child has not changed before
    /// the move. Missing-to-missing is fine for a repository without an index.
    pub fn unchanged(&self) -> bool {
        Self::capture(self.path.clone()).is_some_and(|now| now.fingerprint == self.fingerprint)
    }
}

impl From<std::fs::Metadata> for IndexFingerprint {
    fn from(metadata: std::fs::Metadata) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Self {
                len: metadata.len(),
                device: metadata.dev(),
                inode: metadata.ino(),
                modified_seconds: metadata.mtime(),
                modified_nanoseconds: metadata.mtime_nsec(),
                changed_seconds: metadata.ctime(),
                changed_nanoseconds: metadata.ctime_nsec(),
            }
        }
        #[cfg(not(unix))]
        {
            Self {
                len: metadata.len(),
                modified_nanoseconds: metadata.modified().ok().and_then(|at| {
                    at.duration_since(std::time::UNIX_EPOCH)
                        .ok()
                        .map(|duration| duration.as_nanos())
                }),
            }
        }
    }
}

/// The command a reading runs as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitProbe {
    program: PathBuf,
    args: Vec<OsString>,
    #[cfg(test)]
    candidate_pause: Option<PathBuf>,
}

impl Default for GitProbe {
    fn default() -> Self {
        Self::bridge()
    }
}

impl GitProbe {
    /// The running bridge, as `measure-git`. Under `cargo test` the test
    /// binary stands in, running only [`crate::reclaim::tests`]' reading.
    pub fn bridge() -> Self {
        let program = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("build-bridge"));
        #[cfg(not(test))]
        let args = vec![OsString::from("measure-git")];
        #[cfg(test)]
        let args = [
            "--exact",
            "reclaim::tests::git_reading_child",
            "--nocapture",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        Self {
            program,
            args,
            #[cfg(test)]
            candidate_pause: None,
        }
    }

    /// Any command, for a test that needs a reading that never finishes.
    pub fn command(program: impl Into<PathBuf>, args: &[&str]) -> Self {
        Self {
            program: program.into(),
            args: args.iter().map(OsString::from).collect(),
            #[cfg(test)]
            candidate_pause: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn pause_during_candidate_validation(mut self, marker: &Path) -> Self {
        self.candidate_pause = Some(marker.to_path_buf());
        self
    }

    /// Read `repository`, waiting only while `budget` lasts. `Err` when it
    /// ran out or the daemon stopped first; the reading has been killed and
    /// reaped by then. A reading that could not be started, or said nothing
    /// Build understands, is a repository that could not be read.
    pub fn read(&self, repository: &Path, budget: &Budget) -> Result<GitReading, Unfinished> {
        self.read_with_candidates(repository, &[], budget)
    }

    /// Read a checkout through a descriptor opened beneath the managed root.
    /// The child changes to that held directory before exec, so replacing a
    /// workspace path cannot redirect the reading to a different clone.
    #[cfg(unix)]
    pub fn read_pinned(
        &self,
        checkout_fd: RawFd,
        budget: &Budget,
    ) -> Result<GitReading, Unfinished> {
        self.read_pinned_with_candidates(checkout_fd, &[], budget)
    }

    #[cfg(unix)]
    pub fn read_pinned_with_candidates(
        &self,
        checkout_fd: RawFd,
        candidates: &[PathBuf],
        budget: &Budget,
    ) -> Result<GitReading, Unfinished> {
        self.read_with_candidates_at(Path::new("."), candidates, Some(checkout_fd), budget)
    }

    /// Read Git state and validate all candidate directories with one fresh
    /// index in the same killable child. Candidate paths are repository-relative.
    pub fn read_with_candidates(
        &self,
        repository: &Path,
        candidates: &[PathBuf],
        budget: &Budget,
    ) -> Result<GitReading, Unfinished> {
        self.read_with_candidates_at(repository, candidates, None, budget)
    }

    fn read_with_candidates_at(
        &self,
        repository: &Path,
        candidates: &[PathBuf],
        #[cfg(unix)] checkout_fd: Option<RawFd>,
        #[cfg(not(unix))] _checkout_fd: Option<()>,
        budget: &Budget,
    ) -> Result<GitReading, Unfinished> {
        budget.check()?;
        let mut request_bytes = 2usize;
        for candidate in candidates {
            budget.spend()?;
            budget.check()?;
            request_bytes =
                request_bytes.saturating_add(candidate.as_os_str().len().saturating_mul(6) + 3);
            if request_bytes > MAX_CANDIDATE_REQUEST_BYTES {
                return Err(Unfinished);
            }
        }
        let request = serde_json::to_string(candidates).map_err(|_| Unfinished)?;
        if request.len() > MAX_CANDIDATE_REQUEST_BYTES {
            return Err(Unfinished);
        }
        let mut command = Command::new(&self.program);
        command
            .args(&self.args)
            .env(REPOSITORY_VAR, repository)
            .env(CANDIDATES_VAR, request)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(test)]
        if let Some(marker) = &self.candidate_pause {
            command.env(PAUSE_CANDIDATE_VAR, marker);
        }
        #[cfg(unix)]
        if let Some(fd) = checkout_fd {
            use std::os::unix::process::CommandExt;
            // SAFETY: pre_exec only calls the async-signal-safe fchdir syscall;
            // the caller keeps the descriptor alive until spawn returns.
            unsafe {
                command.pre_exec(move || {
                    if libc::fchdir(fd) == 0 {
                        Ok(())
                    } else {
                        Err(std::io::Error::last_os_error())
                    }
                });
            }
        }
        let spawned = command.spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(error) => {
                eprintln!(
                    "workspace reclaim: read {}: cannot start: {error}",
                    repository.display()
                );
                return Ok(GitReading::default());
            }
        };
        wait_for_reading(&mut child, budget)
    }
}

fn wait_for_reading(
    child: &mut std::process::Child,
    budget: &Budget,
) -> Result<GitReading, Unfinished> {
    let Some(mut stdout) = child.stdout.take() else {
        kill_and_reap(child);
        return Err(Unfinished);
    };
    #[cfg(unix)]
    {
        if set_nonblocking(stdout.as_raw_fd()).is_err() {
            kill_and_reap(child);
            return Err(Unfinished);
        }
        let mut printed = Vec::new();
        loop {
            let eof = match drain_stdout(&mut stdout, &mut printed, budget) {
                Ok(eof) => eof,
                Err(_) => {
                    kill_and_reap(child);
                    return Err(Unfinished);
                }
            };
            let exited = match child.try_wait() {
                Ok(status) => status.is_some(),
                Err(_) => {
                    kill_and_reap(child);
                    return Err(Unfinished);
                }
            };
            if eof && exited {
                break;
            }
            if budget.check().is_err() {
                kill_and_reap(child);
                return Err(Unfinished);
            }
            std::thread::sleep(POLL);
        }
        budget.check()?;
        Ok(std::str::from_utf8(&printed)
            .ok()
            .and_then(parse)
            .unwrap_or_default())
    }
    #[cfg(not(unix))]
    {
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if budget.check().is_err() => {
                    kill_and_reap(child);
                    return Err(Unfinished);
                }
                Ok(None) => std::thread::sleep(POLL),
                Err(_) => {
                    kill_and_reap(child);
                    return Err(Unfinished);
                }
            }
        }
        let mut printed = String::new();
        let _ = stdout.read_to_string(&mut printed);
        budget.check()?;
        Ok(parse(&printed).unwrap_or_default())
    }
}

fn kill_and_reap(child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(unix)]
fn drain_stdout(
    stdout: &mut std::process::ChildStdout,
    printed: &mut Vec<u8>,
    budget: &Budget,
) -> io::Result<bool> {
    let mut chunk = [0u8; 8192];
    loop {
        if budget.check().is_err() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Git reading timed out",
            ));
        }
        match stdout.read(&mut chunk) {
            Ok(0) => return Ok(true),
            Ok(read) => {
                if printed.len().saturating_add(read) > MAX_READING_BYTES {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Git reading too large",
                    ));
                }
                printed.extend_from_slice(&chunk[..read]);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(false),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
}

fn parse(printed: &str) -> Option<GitReading> {
    printed
        .lines()
        // Anywhere in a line: the test harness standing in for the bridge
        // prints the test's name ahead of it.
        .find_map(|line| line.split_once(MARKER).map(|(_, json)| json))
        .and_then(|json| serde_json::from_str(json).ok())
}

/// The reading's side: read the repository named by [`REPOSITORY_VAR`] and
/// answer the line to print.
pub fn reading_line() -> String {
    let candidates = std::env::var(CANDIDATES_VAR)
        .ok()
        .and_then(|json| serde_json::from_str::<Vec<PathBuf>>(&json).ok());
    let reading = std::env::var_os(REPOSITORY_VAR)
        .map(|repository| read_here(Path::new(&repository), candidates.as_deref()))
        .unwrap_or_default();
    format!(
        "{MARKER}{}",
        serde_json::to_string(&reading).unwrap_or_default()
    )
}

/// One repository's reading, in this process: Done's test, how many paths
/// `git status` would list, and when HEAD was committed.
fn read_here(repository: &Path, candidates: Option<&[PathBuf]>) -> GitReading {
    let Ok(summary) = crate::gitgui::work_summary(repository) else {
        return GitReading::default();
    };
    let validation = candidates.and_then(|paths| validate_candidates(repository, paths));
    let index_snapshot = validation.as_ref().and_then(|(index, _)| index.clone());
    let ignore_snapshots = validation
        .as_ref()
        .map(|(_, ignore)| ignore.clone())
        .unwrap_or_default();
    GitReading {
        read: true,
        pushes: summary.pushes,
        behind: summary.behind,
        dirty: summary.dirty,
        dirty_files: dirty_file_count(repository),
        newest_commit_ms: head_commit_ms(repository),
        candidates_valid: validation.is_some(),
        index_snapshot,
        ignore_snapshots,
    }
}

/// One index read for all candidates of this repository. An absent or
/// malformed request fails closed; no candidate can be moved from that read.
fn validate_candidates(
    repository: &Path,
    candidates: &[PathBuf],
) -> Option<(Option<IndexSnapshot>, Vec<IndexSnapshot>)> {
    if candidates.is_empty() {
        return Some((None, Vec::new()));
    }
    let discovered = git2::Repository::open(repository).ok()?;
    let (repo, ignore) = ignore_input_snapshots(&discovered, repository, candidates)?;
    let index_path = std::fs::canonicalize(repo.path()).ok()?.join("index");
    let snapshot = IndexSnapshot::capture(index_path)?;
    let index = repo.index().ok()?;
    let tracked = index
        .iter()
        .map(|entry| artifacts::index_entry_path(entry.path))
        .collect::<Vec<_>>();
    for relative in candidates {
        if !artifacts::valid_relative_candidate(relative)
            || !artifacts::is_build_output(&repo, relative, &tracked)
        {
            return None;
        }
        // Tests pause only after a candidate has actually been validated.
        #[cfg(test)]
        if let Some(marker) = std::env::var_os(PAUSE_CANDIDATE_VAR) {
            let _ = std::fs::write(marker, std::process::id().to_string());
            std::thread::sleep(Duration::from_secs(60));
        }
    }
    if !snapshot.unchanged() || ignore.iter().any(|source| !source.unchanged()) {
        return None;
    }
    Some((Some(snapshot), ignore))
}

/// Files libgit2 may consult while deciding whether these paths are ignored.
/// Config includes are refused: git2 exposes their values but not reliable
/// origin paths for a cheap final stat, so tier-1 pruning fails closed.
fn ignore_input_snapshots(
    discovered: &git2::Repository,
    repository: &Path,
    candidates: &[PathBuf],
) -> Option<(git2::Repository, Vec<IndexSnapshot>)> {
    let gitdir = std::fs::canonicalize(discovered.path()).ok()?;
    let common = std::fs::canonicalize(discovered.commondir()).ok()?;
    let workdir = std::fs::canonicalize(discovered.workdir()?).ok()?;
    let mut paths = BTreeSet::new();
    paths.insert(workdir.join(".git"));
    for directory in [&gitdir, &common] {
        paths.insert(directory.join("config"));
        paths.insert(directory.join("config.worktree"));
        paths.insert(directory.join("commondir"));
        paths.insert(directory.join("info/exclude"));
    }
    for relative in candidates {
        if !artifacts::valid_relative_candidate(relative) {
            return None;
        }
        for ancestor in relative.ancestors() {
            paths.insert(workdir.join(ancestor).join(".gitignore"));
        }
    }
    add_global_ignore_sources(&mut paths)?;
    // Capture fixed inputs before opening a fresh repository or reading its
    // config. A change during either operation then fails the final compare.
    let mut snapshots = paths
        .iter()
        .cloned()
        .map(IndexSnapshot::capture)
        .collect::<Option<Vec<_>>>()?;
    let repo = git2::Repository::open(repository).ok()?;
    if std::fs::canonicalize(repo.path()).ok()? != gitdir
        || std::fs::canonicalize(repo.commondir()).ok()? != common
        || std::fs::canonicalize(repo.workdir()?).ok()? != workdir
    {
        return None;
    }
    let config = standalone_config(&repo)?;
    match config.get_path("core.excludesfile") {
        Ok(path) => {
            if !path.is_absolute() {
                return None;
            }
            if paths.insert(path.clone()) {
                snapshots.push(IndexSnapshot::capture(path)?);
            }
        }
        Err(error) if error.code() == git2::ErrorCode::NotFound => {}
        Err(_) => return None,
    }
    Some((repo, snapshots))
}

fn standalone_config(repo: &git2::Repository) -> Option<git2::Config> {
    let config = repo.config().ok()?;
    let mut entries = config.entries(None).ok()?;
    while let Some(entry) = entries.next() {
        let entry = entry.ok()?;
        let name = entry.name_bytes().to_ascii_lowercase();
        if entry.include_depth() > 0
            || name.starts_with(b"include.")
            || name.starts_with(b"includeif.")
        {
            return None;
        }
    }
    drop(entries);
    Some(config)
}

fn add_global_ignore_sources(paths: &mut BTreeSet<PathBuf>) -> Option<()> {
    #[cfg(unix)]
    paths.insert(PathBuf::from("/etc/gitconfig"));
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if let Some(home) = home {
        if !home.is_absolute() {
            return None;
        }
        paths.insert(home.join(".gitconfig"));
        paths.insert(home.join(".config/git/config"));
        paths.insert(home.join(".config/git/ignore"));
    }
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from) {
        if !xdg.is_absolute() {
            return None;
        }
        paths.insert(xdg.join("git/config"));
        paths.insert(xdg.join("git/ignore"));
    }
    for variable in ["GIT_CONFIG_GLOBAL", "GIT_CONFIG_SYSTEM"] {
        if let Some(path) = std::env::var_os(variable).map(PathBuf::from) {
            if !path.is_absolute() {
                return None;
            }
            paths.insert(path);
        }
    }
    for path in [
        git2::Config::find_global(),
        git2::Config::find_xdg(),
        git2::Config::find_system(),
    ]
    .into_iter()
    .flatten()
    {
        if !path.is_absolute() {
            return None;
        }
        paths.insert(path);
    }
    Some(())
}

/// How many paths `git status` would list: edits, staged or not, and untracked
/// files. Ignored files are not work.
fn dirty_file_count(repository: &Path) -> u64 {
    let Ok(repo) = git2::Repository::open(repository) else {
        return 0;
    };
    let mut options = git2::StatusOptions::new();
    options
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(false);
    repo.statuses(Some(&mut options))
        .map(|statuses| statuses.len() as u64)
        .unwrap_or(0)
}

fn head_commit_ms(repository: &Path) -> Option<i64> {
    let repo = git2::Repository::open(repository).ok()?;
    let commit = repo.head().ok()?.peel_to_commit().ok()?;
    Some(commit.time().seconds().saturating_mul(1000))
}
