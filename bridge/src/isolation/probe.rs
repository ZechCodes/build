//! Side-effect-free Rift capability detection, plus an opt-in real-backend
//! test helper. Availability only inspects paths and asks the executable for
//! help; it never initializes a workspace or probes reflinks itself.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::git_process::run_command_with_deadline;

const PROBE_DEADLINE: Duration = Duration::from_secs(2);

pub fn rift_availability(project: &Path, worktrees_root: &Path) -> Result<(), String> {
    rift_availability_with(OsStr::new("rift"), project, worktrees_root)
}

pub(crate) fn rift_availability_with(
    executable: &OsStr,
    project: &Path,
    worktrees_root: &Path,
) -> Result<(), String> {
    validate_paths(project, worktrees_root)?;
    require_help(
        executable,
        project,
        &["--help"],
        &["init", "create", "remove", "list", "gc"],
    )?;
    require_help(executable, project, &["init", "--help"], &["--here"])?;
    require_help(
        executable,
        project,
        &["create", "--help"],
        &["--name", "--into", "--copy-all", "--no-hooks"],
    )?;
    require_help(executable, project, &["remove", "--help"], &["--no-hooks"])
}

pub(crate) fn validate_paths(project: &Path, worktrees_root: &Path) -> Result<(), String> {
    let project = std::fs::canonicalize(project)
        .map_err(|error| format!("the project cannot be opened for Rift ({error})"))?;
    if !has_independent_git_dir(&project) {
        return Err(
            "Rift requires the project to be a standalone Git repository, not a linked worktree"
                .to_string(),
        );
    }
    let worktrees_root = resolved_path(worktrees_root)?;
    if worktrees_root.starts_with(&project) {
        return Err("Rift checkout storage must be outside the project being copied".to_string());
    }
    let ancestor = worktrees_root
        .ancestors()
        .find(|ancestor| ancestor.exists())
        .ok_or_else(|| "Rift checkout storage has no existing parent directory".to_string())?;
    if !ancestor.is_dir() {
        return Err(format!(
            "Rift checkout storage cannot be created under {}",
            ancestor.display()
        ));
    }
    Ok(())
}

pub(crate) fn has_independent_git_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path.join(".git")).is_ok_and(|metadata| metadata.file_type().is_dir())
}

pub(crate) fn validate_database_path(project: &Path, database: &Path) -> Result<(), String> {
    let project = std::fs::canonicalize(project)
        .map_err(|error| format!("the project cannot be opened for Rift ({error})"))?;
    let parent = database
        .parent()
        .ok_or_else(|| "Rift registry path has no parent directory".to_string())?;
    if resolved_path(parent)?.starts_with(project) {
        return Err("Rift registry must be outside the project being copied".to_string());
    }
    Ok(())
}

fn resolved_path(path: &Path) -> Result<PathBuf, String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|current| current.join(path))
            .map_err(|error| format!("Rift checkout storage cannot be resolved ({error})"))?
    };
    if absolute
        .components()
        .any(|component| component == std::path::Component::ParentDir)
    {
        return Err("Rift paths cannot contain parent-directory components".to_string());
    }
    let normalized = normalize(&absolute);
    let existing = normalized
        .ancestors()
        .find(|ancestor| ancestor.exists())
        .ok_or_else(|| "Rift checkout storage has no existing parent directory".to_string())?;
    let canonical = std::fs::canonicalize(existing)
        .map_err(|error| format!("Rift checkout storage cannot be resolved ({error})"))?;
    let suffix = normalized
        .strip_prefix(existing)
        .expect("an ancestor is always a path prefix");
    Ok(canonical.join(suffix))
}

fn normalize(path: &Path) -> PathBuf {
    use std::path::Component;

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
}

fn require_help(
    executable: &OsStr,
    dir: &Path,
    string_args: &[&str],
    capabilities: &[&str],
) -> Result<(), String> {
    let owned: Vec<OsString> = string_args.iter().map(OsString::from).collect();
    let args: Vec<&OsStr> = owned.iter().map(OsString::as_os_str).collect();
    let output = run_command_with_deadline(executable, dir, &args, PROBE_DEADLINE)
        .map_err(|error| format!("the Rift CLI is unavailable ({error})"))?;
    if !output.status.success() {
        return Err(format!("the Rift CLI cannot run {}", string_args.join(" ")));
    }
    let help = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let missing: Vec<&&str> = capabilities
        .iter()
        .filter(|capability| !help.contains(**capability))
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "the installed Rift CLI lacks required capabilities: {}",
            missing.into_iter().copied().collect::<Vec<_>>().join(", ")
        ))
    }
}

/// Exercise the installed Rift executable against a disposable repository.
/// Real-backend tests use this instead of making platform-specific reflink
/// assumptions. A failure prints why the test is skipped.
#[cfg(test)]
pub(crate) fn rift_or_skip(dir: &Path) -> bool {
    use crate::git_fixture::init_repo_named;
    use crate::isolation::{IsolationBackend, RiftBackend};

    let fixture = tempfile::tempdir_in(dir).unwrap();
    let project = init_repo_named(fixture.path(), "rift-probe-project");
    let worktrees_root = fixture.path().join("worktrees");
    if let Err(reason) = rift_availability(&project, &worktrees_root) {
        eprintln!("skipping Rift test: {reason}");
        return false;
    }
    let backend = RiftBackend::new(&worktrees_root);
    let checkout = worktrees_root.join("probe");
    let result = backend
        .materialize(&project, "main", &checkout)
        .and_then(|()| backend.remove(&project, &checkout));
    match result {
        Ok(()) => true,
        Err(error) => {
            eprintln!("skipping Rift test: {error}");
            false
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::git_fixture::init_repo;
    use std::os::unix::fs::PermissionsExt;

    fn fake_rift(dir: &Path, body: &str) -> PathBuf {
        let executable = dir.join("rift");
        std::fs::write(&executable, body).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        executable
    }

    #[test]
    fn availability_validates_help_without_creating_storage_or_a_database() {
        let (dir, project) = init_repo();
        let root = dir.path().join("outside").join("worktrees");
        let executable = fake_rift(
            dir.path(),
            "#!/bin/sh\nprintf '%s\\n' 'init create remove list gc --here --name --into --copy-all --no-hooks'\n",
        );

        rift_availability_with(executable.as_os_str(), &project, &root).unwrap();

        assert!(!root.exists());
        assert!(!project.join(".rift").exists());
    }

    #[test]
    fn availability_rejects_an_incomplete_cli() {
        let (dir, project) = init_repo();
        let executable = fake_rift(
            dir.path(),
            "#!/bin/sh\nprintf 'init create remove list gc\\n'\n",
        );

        let reason = rift_availability_with(
            executable.as_os_str(),
            &project,
            &dir.path().join("worktrees"),
        )
        .unwrap_err();

        assert!(reason.contains("lacks required capabilities"), "{reason}");
    }

    #[test]
    fn availability_rejects_storage_inside_the_project() {
        let (dir, project) = init_repo();
        let executable = fake_rift(dir.path(), "#!/bin/sh\nexit 0\n");

        let reason =
            rift_availability_with(executable.as_os_str(), &project, &project.join("worktrees"))
                .unwrap_err();

        assert!(reason.contains("outside the project"), "{reason}");
    }

    #[test]
    fn availability_resolves_a_symlink_before_checking_storage_ownership() {
        let (dir, project) = init_repo();
        let link = dir.path().join("project-link");
        std::os::unix::fs::symlink(&project, &link).unwrap();
        let executable = fake_rift(dir.path(), "#!/bin/sh\nexit 0\n");

        let reason =
            rift_availability_with(executable.as_os_str(), &project, &link.join("worktrees"))
                .unwrap_err();

        assert!(reason.contains("outside the project"), "{reason}");
    }

    #[test]
    fn availability_rejects_a_symlinked_git_directory() {
        let (dir, project) = init_repo();
        let linked = dir.path().join("linked-git");
        std::fs::create_dir(&linked).unwrap();
        std::os::unix::fs::symlink(project.join(".git"), linked.join(".git")).unwrap();
        let executable = fake_rift(dir.path(), "#!/bin/sh\nexit 0\n");

        let reason = rift_availability_with(
            executable.as_os_str(),
            &linked,
            &dir.path().join("worktrees"),
        )
        .unwrap_err();

        assert!(reason.contains("standalone Git repository"), "{reason}");
    }
}
