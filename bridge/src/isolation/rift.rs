//! The Rift CLI backend. Rift owns filesystem copy-on-write and its registry;
//! Build owns branch lifecycle, provenance, and the shape of each checkout.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;

use super::{
    checkout_name, directory_name, local_branch_ref, rift_marker_names, teardown_in_git_dir,
    write_rift_marker, BranchTeardown, Isolation, IsolationBackend, WorktreeError,
};
use crate::git_process::{git_failure, run_command_with_deadline, run_git, run_git_with_deadline};

const RIFT_DEADLINE: Duration = Duration::from_secs(30);
const RIFT_MATERIALIZE_DEADLINE: Duration = Duration::from_secs(300);

/// A Rift installation scoped to Build's worktree root. Its private database
/// prevents Build's cleanup and discovery from touching a user's own Rift
/// registry.
#[derive(Clone, Debug)]
pub struct RiftBackend {
    worktrees_root: PathBuf,
    database_path: PathBuf,
    executable: OsString,
}

impl RiftBackend {
    pub fn new(worktrees_root: impl Into<PathBuf>) -> Self {
        let worktrees_root = worktrees_root.into();
        Self::with_registry_root(&worktrees_root, &worktrees_root, OsString::from("rift"))
    }

    /// Injecting the executable makes command behavior deterministic in unit
    /// tests without changing process-global PATH.
    #[cfg(test)]
    pub(crate) fn with_executable(
        worktrees_root: impl Into<PathBuf>,
        executable: impl Into<OsString>,
    ) -> Self {
        let worktrees_root = worktrees_root.into();
        Self::with_registry_root(&worktrees_root, &worktrees_root, executable)
    }

    pub(crate) fn with_registry_root(
        worktrees_root: impl Into<PathBuf>,
        registry_root: impl Into<PathBuf>,
        executable: impl Into<OsString>,
    ) -> Self {
        Self {
            worktrees_root: worktrees_root.into(),
            database_path: registry_root.into().join(".rift").join("registry.sqlite"),
            executable: executable.into(),
        }
    }

    #[cfg(test)]
    pub(crate) fn database_path(&self) -> &Path {
        &self.database_path
    }

    fn database_exists(&self) -> bool {
        self.database_path.is_file()
    }

    fn ensure_database_parent(&self) -> Result<(), WorktreeError> {
        let parent = self.database_path.parent().ok_or_else(|| {
            WorktreeError::Refused("Rift registry path has no parent directory".to_string())
        })?;
        std::fs::create_dir_all(parent)?;
        Ok(())
    }

    fn command_args(&self, command: &[&OsStr]) -> Vec<OsString> {
        let mut owned = vec![
            OsString::from("--database"),
            self.database_path.as_os_str().to_owned(),
        ];
        owned.extend(command.iter().map(|arg| (*arg).to_owned()));
        owned
    }

    fn run_with_deadline(
        &self,
        dir: &Path,
        command: &[&OsStr],
        deadline: Duration,
    ) -> Result<Output, WorktreeError> {
        let owned = self.command_args(command);
        let args: Vec<&OsStr> = owned.iter().map(OsString::as_os_str).collect();
        let output = run_command_with_deadline(&self.executable, dir, &args, deadline)?;
        if !output.status.success() {
            return Err(rift_failure(&args, &output));
        }
        Ok(output)
    }

    fn run(&self, dir: &Path, command: &[&OsStr]) -> Result<Output, WorktreeError> {
        self.run_with_deadline(dir, command, RIFT_DEADLINE)
    }

    fn run_optional(
        &self,
        dir: &Path,
        command: &[&OsStr],
    ) -> Result<Option<Output>, WorktreeError> {
        match self.run(dir, command) {
            Ok(output) => Ok(Some(output)),
            Err(WorktreeError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    fn list(&self, project: &Path) -> Result<Vec<PathBuf>, WorktreeError> {
        if !self.database_exists() || !project.join(".rift").is_file() {
            return Ok(Vec::new());
        }
        let output = match self.run_optional(project, &[OsStr::new("list"), project.as_os_str()]) {
            Ok(Some(output)) => output,
            Ok(None) => return Ok(Vec::new()),
            Err(error) => {
                eprintln!(
                    "Rift registry list skipped for {}: {error}",
                    project.display()
                );
                return Ok(Vec::new());
            }
        };
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(PathBuf::from)
            .collect())
    }

    fn gc(&self, project: &Path) -> Result<(), WorktreeError> {
        if self.database_exists() {
            self.run_optional(project, &[OsStr::new("gc")])?;
        }
        Ok(())
    }

    fn cleanup_created(&self, project: &Path, path: &Path) -> Result<(), WorktreeError> {
        if path.exists() {
            self.run(
                project,
                &[
                    OsStr::new("remove"),
                    OsStr::new("--no-hooks"),
                    path.as_os_str(),
                ],
            )?;
        }
        self.gc(project)
    }

    fn failed_create(&self, project: &Path, path: &Path, original: WorktreeError) -> WorktreeError {
        match self.cleanup_created(project, path) {
            Ok(()) => original,
            Err(cleanup) => WorktreeError::Refused(format!(
                "Rift checkout creation failed ({original}); cleanup also failed ({cleanup})"
            )),
        }
    }

    /// Check whether this configured Rift executable can copy an ordinary
    /// directory into this backend's root.
    pub(crate) fn directory_availability(&self, source: &Path) -> Result<(), String> {
        super::probe::rift_directory_availability_with(
            &self.executable,
            source,
            &self.worktrees_root,
        )
    }

    /// Materialize an ordinary directory through the configured Rift CLI.
    /// The source is not required to be a Git repository and the destination
    /// must be one direct child of this backend's root.
    pub(crate) fn materialize_directory(
        &self,
        source: &Path,
        destination: &Path,
    ) -> Result<(), WorktreeError> {
        let marker = source.join(".rift");
        let marker_existed = marker.exists();
        self.materialize_directory_inner(source, destination)
            .inspect_err(|_| {
                if !marker_existed {
                    let _ = std::fs::remove_file(&marker);
                }
            })
    }

    fn materialize_directory_inner(
        &self,
        source: &Path,
        destination: &Path,
    ) -> Result<(), WorktreeError> {
        if destination.exists() {
            return Err(WorktreeError::Refused(format!(
                "{} already exists",
                destination.display()
            )));
        }
        let root = &self.worktrees_root;
        super::probe::validate_storage_paths(source, root).map_err(WorktreeError::Refused)?;
        if destination.parent() != Some(root) {
            return Err(WorktreeError::Refused(format!(
                "Rift directory {} is outside its configured workspaces root {}",
                destination.display(),
                root.display()
            )));
        }
        super::probe::validate_database_path(source, &self.database_path)
            .map_err(WorktreeError::Refused)?;
        self.ensure_database_parent()?;
        super::probe::validate_storage_paths(source, root).map_err(WorktreeError::Refused)?;
        super::probe::validate_database_path(source, &self.database_path)
            .map_err(WorktreeError::Refused)?;
        self.run_with_deadline(
            source,
            &[OsStr::new("init"), source.as_os_str(), OsStr::new("--here")],
            RIFT_MATERIALIZE_DEADLINE,
        )?;
        // `--name` is the directory Rift is told to make under `--into`, not
        // the name the record is keyed by, so it stays the path's last segment.
        let name = directory_name(destination).ok_or_else(|| {
            WorktreeError::Refused(format!(
                "directory path has no name: {}",
                destination.display()
            ))
        })?;
        let created = match self.run_with_deadline(
            source,
            &[
                OsStr::new("create"),
                source.as_os_str(),
                OsStr::new("--into"),
                root.as_os_str(),
                OsStr::new("--name"),
                OsStr::new(&name),
                OsStr::new("--copy-all"),
                OsStr::new("--no-hooks"),
            ],
            RIFT_MATERIALIZE_DEADLINE,
        ) {
            Ok(created) => created,
            Err(error) => return Err(self.failed_create(source, destination, error)),
        };
        let reported = PathBuf::from(String::from_utf8_lossy(&created.stdout).trim());
        let expected = std::fs::canonicalize(destination)
            .map_err(|error| self.failed_create(source, destination, error.into()))?;
        let reported = std::fs::canonicalize(&reported)
            .map_err(|error| self.failed_create(source, destination, error.into()))?;
        if reported != expected {
            return Err(self.failed_create(
                source,
                destination,
                WorktreeError::Refused(format!(
                    "Rift created {} instead of {}",
                    reported.display(),
                    destination.display()
                )),
            ));
        }
        Ok(())
    }
}

impl IsolationBackend for RiftBackend {
    fn kind(&self) -> Isolation {
        Isolation::Rift
    }

    fn materialize(&self, project: &Path, branch: &str, path: &Path) -> Result<(), WorktreeError> {
        if path.exists() {
            return Err(WorktreeError::Refused(format!(
                "{} already exists",
                path.display()
            )));
        }
        refuse_if_mid_operation(project)?;
        let worktrees_root = &self.worktrees_root;
        super::probe::validate_paths(project, worktrees_root).map_err(WorktreeError::Refused)?;
        if path.parent() != Some(worktrees_root) {
            return Err(WorktreeError::Refused(format!(
                "Rift checkout {} is outside its configured worktrees root {}",
                path.display(),
                worktrees_root.display()
            )));
        }
        super::probe::validate_database_path(project, &self.database_path)
            .map_err(WorktreeError::Refused)?;
        self.ensure_database_parent()?;
        // Re-check after creating the registry parent so symlink races cannot
        // place Rift's database inside the project that `init` may convert.
        super::probe::validate_paths(project, worktrees_root).map_err(WorktreeError::Refused)?;
        super::probe::validate_database_path(project, &self.database_path)
            .map_err(WorktreeError::Refused)?;
        self.run_with_deadline(
            project,
            &[
                OsStr::new("init"),
                project.as_os_str(),
                OsStr::new("--here"),
            ],
            RIFT_MATERIALIZE_DEADLINE,
        )?;

        let parent = path.parent().ok_or_else(|| {
            WorktreeError::Refused(format!("checkout path has no parent: {}", path.display()))
        })?;
        // As above: Rift creates `<parent>/<name>`, so this is the directory
        // the caller asked for. Its record is found again by path, and read
        // back under `checkout_name` in `holds_record` and `teardown_record`.
        let name = directory_name(path).ok_or_else(|| {
            WorktreeError::Refused(format!(
                "checkout path has no directory name: {}",
                path.display()
            ))
        })?;
        let created = self.run_with_deadline(
            project,
            &[
                OsStr::new("create"),
                project.as_os_str(),
                OsStr::new("--into"),
                parent.as_os_str(),
                OsStr::new("--name"),
                OsStr::new(&name),
                OsStr::new("--copy-all"),
                OsStr::new("--no-hooks"),
            ],
            RIFT_MATERIALIZE_DEADLINE,
        )?;
        let reported = PathBuf::from(String::from_utf8_lossy(&created.stdout).trim());
        let expected = match std::fs::canonicalize(path) {
            Ok(expected) => expected,
            Err(error) => {
                return Err(self.failed_create(project, path, error.into()));
            }
        };
        let reported = match std::fs::canonicalize(&reported) {
            Ok(reported) => reported,
            Err(error) => {
                return Err(self.failed_create(project, path, error.into()));
            }
        };
        if reported != expected {
            let error = WorktreeError::Refused(format!(
                "Rift created {} instead of {}",
                reported.display(),
                path.display()
            ));
            return Err(self.failed_create(project, path, error));
        }
        if !super::probe::has_independent_git_dir(path) {
            let error = WorktreeError::Refused(format!(
                "Rift created {} without an independent Git directory",
                path.display()
            ));
            return Err(self.failed_create(project, path, error));
        }
        match populate_rift(project, branch, path) {
            Ok(()) => Ok(()),
            Err(error) => Err(self.failed_create(project, path, error)),
        }
    }

    fn verify(&self, project: &Path, path: &Path, branch: &str) -> Result<(), WorktreeError> {
        if !rift_marker_names(path, project) {
            return Err(WorktreeError::Refused(format!(
                "{} is not a Rift checkout of this project",
                path.display()
            )));
        }
        if !path.join(".rift").is_file() {
            return Err(WorktreeError::Refused(format!(
                "{} is missing its Rift marker",
                path.display()
            )));
        }
        let head = run_git(path, &["symbolic-ref", "--short", "HEAD"])?;
        if head.trim() != branch {
            return Err(WorktreeError::Refused(format!(
                "the Rift checkout is on {} not {branch}",
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

    fn remove(&self, project: &Path, path: &Path) -> Result<(), WorktreeError> {
        if !path.exists() {
            return self.gc(project);
        }
        if Isolation::of(path) != Some(Isolation::Rift) {
            return Ok(());
        }
        if !rift_marker_names(path, project) {
            return Err(WorktreeError::Refused(format!(
                "{} is not a Rift checkout of this project",
                path.display()
            )));
        }
        if !self.database_exists() {
            return Err(WorktreeError::Refused(format!(
                "Rift registry is missing for {}",
                path.display()
            )));
        }
        self.run(
            project,
            &[
                OsStr::new("remove"),
                OsStr::new("--no-hooks"),
                path.as_os_str(),
            ],
        )?;
        self.gc(project)
    }

    fn discover(
        &self,
        project: &Path,
        worktrees_root: &Path,
    ) -> Result<Vec<PathBuf>, WorktreeError> {
        let canonical_root = match std::fs::canonicalize(worktrees_root) {
            Ok(root) => root,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut candidates = self.list(project)?;
        if let Ok(entries) = std::fs::read_dir(worktrees_root) {
            candidates.extend(entries.filter_map(Result::ok).map(|entry| entry.path()));
        }
        let mut found = Vec::new();
        for listed in candidates {
            let Ok(path) = std::fs::canonicalize(listed) else {
                continue;
            };
            if path.starts_with(&canonical_root)
                && rift_marker_names(&path, project)
                && !found.contains(&path)
            {
                found.push(path);
            }
        }
        Ok(found)
    }

    fn prune(&self, project: &Path) -> Result<(), WorktreeError> {
        self.gc(project)
    }

    fn holds_record(&self, project: &Path, name: &str) -> Result<bool, WorktreeError> {
        Ok(self
            .list(project)?
            .iter()
            .any(|path| checkout_name(path).as_deref() == Some(name)))
    }

    fn teardown_record(
        &self,
        project: &Path,
        name: &str,
    ) -> Result<Option<BranchTeardown>, WorktreeError> {
        let Some(path) = self
            .list(project)?
            .into_iter()
            .find(|path| checkout_name(path).as_deref() == Some(name))
        else {
            return Ok(None);
        };
        if !rift_marker_names(&path, project) {
            return Ok(None);
        }
        teardown_in_git_dir(&path.join(".git")).map(Some)
    }
}

fn rift_failure(args: &[&OsStr], output: &Output) -> WorktreeError {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let detail = [stderr.trim(), stdout.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    WorktreeError::Command(format!("rift {args:?}: {detail}"))
}

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
        return Err(git_failure(&args, &output).into());
    }
    Ok(())
}

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

fn populate_rift(project: &Path, branch: &str, path: &Path) -> Result<(), WorktreeError> {
    let repository = git2::Repository::open(path)?;
    let workdir = repository.workdir().ok_or_else(|| {
        WorktreeError::Refused(format!("Rift checkout {} is bare", path.display()))
    })?;
    if std::fs::canonicalize(workdir)? != std::fs::canonicalize(path)? {
        return Err(WorktreeError::Refused(format!(
            "Rift checkout {} has a Git worktree outside the checkout",
            path.display()
        )));
    }
    let git_dir = path.join(".git");
    let inherited_records = git_dir.join("worktrees");
    if inherited_records.exists() {
        std::fs::remove_dir_all(&inherited_records)?;
    }
    let index_lock = git_dir.join("index.lock");
    if index_lock.exists() {
        std::fs::remove_file(&index_lock)?;
    }
    write_rift_marker(path, project)?;

    let head_ref = local_branch_ref(branch);
    run_git(path, &["symbolic-ref", "HEAD", &head_ref])?;
    run_git(path, &["reset", "--hard"])?;
    run_git(path, &["clean", "-fd"])?;
    verify_head_on_tip(project, branch, path)
}

fn verify_head_on_tip(project: &Path, branch: &str, path: &Path) -> Result<(), WorktreeError> {
    let checkout = git2::Repository::open(path)?;
    let checkout_head = checkout.head()?.peel_to_commit()?.id();
    let project = git2::Repository::open(project)?;
    let tip = project
        .find_reference(&local_branch_ref(branch))?
        .peel_to_commit()?
        .id();
    if checkout_head != tip {
        return Err(WorktreeError::Refused(format!(
            "Rift checkout HEAD {checkout_head} is not the {branch} tip {tip}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_fixture::init_repo;

    #[test]
    fn registry_is_private_to_the_worktrees_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("worktrees");
        let backend = RiftBackend::new(&root);

        assert_eq!(
            backend.database_path(),
            root.join(".rift").join("registry.sqlite")
        );
    }

    #[test]
    fn a_missing_private_registry_has_no_records_and_runs_no_cli() {
        let (dir, project) = init_repo();
        let missing = dir.path().join("never-exists");
        let backend = RiftBackend::with_executable(dir.path().join("worktrees"), &missing);

        assert!(!backend.holds_record(&project, "task").unwrap());
        assert!(backend.discover(&project, dir.path()).unwrap().is_empty());
        backend.prune(&project).unwrap();
    }

    #[test]
    fn missing_cli_keeps_optional_walks_working_and_refuses_live_removal() {
        let (dir, project) = init_repo();
        let root = dir.path().join("worktrees");
        let registry_parent = root.join(".rift");
        std::fs::create_dir_all(&registry_parent).unwrap();
        std::fs::write(registry_parent.join("registry.sqlite"), "private registry").unwrap();
        std::fs::write(project.join(".rift"), "project-rift").unwrap();
        let checkout = root.join("task");
        std::fs::create_dir_all(checkout.join(".git")).unwrap();
        std::fs::write(checkout.join(".rift"), "checkout-rift").unwrap();
        write_rift_marker(&checkout, &project).unwrap();
        let missing = dir.path().join("never-exists");
        let backend = RiftBackend::with_executable(&root, missing);

        assert!(!backend.holds_record(&project, "task").unwrap());
        backend.prune(&project).unwrap();
        backend.remove(&project, &root.join("gone")).unwrap();
        assert_eq!(
            backend.discover(&project, &root).unwrap(),
            vec![std::fs::canonicalize(&checkout).unwrap()]
        );

        let error = backend.remove(&project, &checkout).unwrap_err();
        assert!(matches!(error, WorktreeError::Io(_)), "{error}");
        assert!(
            checkout.exists(),
            "failed removal must preserve the checkout"
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_private_record_never_blocks_worktree_record_walks() {
        let (dir, project) = init_repo();
        let root = dir.path().join("worktrees");
        let registry_parent = root.join(".rift");
        std::fs::create_dir_all(&registry_parent).unwrap();
        std::fs::write(registry_parent.join("registry.sqlite"), "private registry").unwrap();
        std::fs::write(project.join(".rift"), "foreign marker").unwrap();
        let checkout = root.join("visible-on-disk");
        std::fs::create_dir_all(checkout.join(".git")).unwrap();
        std::fs::write(checkout.join(".rift"), "checkout-rift").unwrap();
        write_rift_marker(&checkout, &project).unwrap();
        let backend = RiftBackend::with_executable(&root, "/bin/false");

        assert!(!backend.holds_record(&project, "unknown-record").unwrap());
        assert_eq!(
            backend.discover(&project, &root).unwrap(),
            vec![std::fs::canonicalize(checkout).unwrap()]
        );
    }

    #[test]
    fn a_foreign_build_marker_is_refused_before_the_cli_runs() {
        let (dir, project) = init_repo();
        let other = dir.path().join("other");
        std::fs::create_dir_all(other.join(".git")).unwrap();
        std::fs::write(other.join(".rift"), "foreign-rift\n").unwrap();
        write_rift_marker(&other, dir.path()).unwrap();
        let backend = RiftBackend::with_executable(dir.path().join("worktrees"), "missing-rift");

        let error = backend.remove(&project, &other).unwrap_err().to_string();

        assert!(
            error.contains("not a Rift checkout of this project"),
            "{error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_registry_symlink_into_the_project_is_refused_before_init() {
        let (dir, project) = init_repo();
        let root = dir.path().join("worktrees");
        std::fs::create_dir(&root).unwrap();
        std::os::unix::fs::symlink(&project, root.join(".rift")).unwrap();
        let backend = RiftBackend::with_executable(&root, "missing-rift");

        let error = backend
            .materialize(&project, "main", &root.join("task"))
            .unwrap_err()
            .to_string();

        assert!(error.contains("registry must be outside"), "{error}");
    }
}

#[cfg(all(test, unix))]
#[path = "rift_tests.rs"]
mod cli_tests;
