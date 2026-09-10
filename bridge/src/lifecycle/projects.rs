use crate::lifecycle::{OpenedRepository, RemoteChanged};
use crate::lifecycle::{Performed, WorktreeChange, WorktreeMutation};
use crate::worktree::{git_default_branch, git_in, git_remote_origin, remotes_match};
use std::path::PathBuf;

/// canonical path, the base branch — the one named, or the one the checkout is
/// standing on — proved to resolve, and the `origin` it is wired to.
///
/// The one place a project's facts are read, whichever door reached the
/// directory: opened where it stands, cloned into the projects folder, or
/// created from nothing.
fn open_repo(path: PathBuf, requested_base: Option<String>) -> Result<OpenedRepository, String> {
    let path = std::fs::canonicalize(&path).unwrap_or(path);
    let repo =
        git2::Repository::open(&path).map_err(|error| format!("not a git repository: {error}"))?;
    let base = requested_base
        .or_else(|| git_default_branch(&path))
        .unwrap_or_else(|| "main".to_string());
    repo.revparse_single(&base)
        .map_err(|_| format!("base branch '{base}' not found in repo"))?;
    Ok(OpenedRepository {
        remote: git_remote_origin(&path),
        path,
        base,
        created_checkout: None,
    })
}

/// One repository's registration, once its directory is on disk. Every project
/// door ends here, so what a project knows about itself is read in one place.
fn opened(
    path: PathBuf,
    requested_base: Option<String>,
    created: bool,
) -> Result<Performed<OpenedRepository>, String> {
    let mut registration = match open_repo(path.clone(), requested_base) {
        Ok(registration) => registration,
        Err(error) => {
            if created {
                let _ = std::fs::remove_dir_all(&path);
            }
            return Err(error);
        }
    };
    registration.created_checkout = created.then_some(path);
    Ok(Performed {
        change: WorktreeChange::nothing(),
        output: registration,
    })
}

/// `project.add` — register a repository where the user already keeps it.
pub struct OpenRepo {
    pub path: PathBuf,
    pub requested_base: Option<String>,
}

impl WorktreeMutation for OpenRepo {
    type Output = OpenedRepository;
    fn perform(self) -> Result<Performed<Self::Output>, String> {
        opened(self.path, self.requested_base, false)
    }
}

/// `project.clone` — put a repository in the projects folder and register it,
/// or register the one already standing there when it is the same repository.
pub struct CloneRepo {
    pub url: String,
    pub name: String,
    pub dest: PathBuf,
    pub projects_dir: PathBuf,
    pub requested_base: Option<String>,
}

impl WorktreeMutation for CloneRepo {
    type Output = OpenedRepository;
    fn perform(self) -> Result<Performed<Self::Output>, String> {
        if self.dest.exists() {
            if !self.dest.join(".git").exists() {
                return Err(format!(
                    "'{}' already exists in the projects folder and is not a git repo",
                    self.name
                ));
            }
            if let Some(origin) = git_remote_origin(&self.dest) {
                if !remotes_match(&origin, &self.url) {
                    return Err(format!(
                        "'{}' already exists with a different remote ({origin})",
                        self.name
                    ));
                }
            }
            return opened(self.dest, self.requested_base, false);
        }
        std::fs::create_dir_all(&self.projects_dir)
            .map_err(|error| format!("cannot create projects folder: {error}"))?;
        let cloned = std::process::Command::new("git")
            .arg("clone")
            .arg(&self.url)
            .arg(&self.dest)
            .output()
            .map_err(|error| format!("could not run git: {error}"))?;
        if !cloned.status.success() {
            // A clone that got far enough to make the directory leaves nothing
            // behind: a retry has to find the same empty folder this one did.
            let _ = std::fs::remove_dir_all(&self.dest);
            return Err(format!(
                "git clone failed: {}",
                String::from_utf8_lossy(&cloned.stderr).trim()
            ));
        }
        opened(self.dest, self.requested_base, true)
    }
}

/// `project.create` — make a repository from nothing and register it, with an
/// initial commit so its base branch resolves and work can dispatch into it.
pub struct CreateRepo {
    pub name: String,
    /// Where the repository lands — the same path the decide phase reserved its
    /// row under, so what is guarded and what is written are one fact.
    pub dest: PathBuf,
    pub base_branch: String,
    pub remote: Option<String>,
}

impl CreateRepo {
    fn write(&self, dest: &std::path::Path) -> Result<(), String> {
        let parent = dest
            .parent()
            .ok_or_else(|| format!("cannot create {}: it has no parent", dest.display()))?;
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
        if dest.exists() {
            return Err(format!(
                "'{}' already exists in {}",
                self.name,
                parent.display()
            ));
        }
        std::fs::create_dir_all(dest)
            .map_err(|error| format!("cannot create {}: {error}", self.name))?;
        git_in(dest, &["init", "-b", &self.base_branch])?;
        std::fs::write(dest.join("README.md"), format!("# {}\n", self.name))
            .map_err(|error| format!("cannot write README: {error}"))?;
        git_in(dest, &["add", "."])?;
        // Commit with an explicit identity so it never depends on host git config.
        git_in(
            dest,
            &[
                "-c",
                "user.email=build@build.ing",
                "-c",
                "user.name=Build",
                "commit",
                "-m",
                "Initial commit",
            ],
        )?;
        if let Some(remote) = &self.remote {
            git_in(dest, &["remote", "add", "origin", remote])?;
        }
        Ok(())
    }
}

impl WorktreeMutation for CreateRepo {
    type Output = OpenedRepository;
    fn perform(self) -> Result<Performed<Self::Output>, String> {
        let dest = self.dest.clone();
        let existed = dest.exists();
        if let Err(error) = self.write(&dest) {
            // Half a repository is worse than none: the retry has to start
            // where this one did. A directory that was already there is not
            // this call's to remove.
            if !existed {
                let _ = std::fs::remove_dir_all(&dest);
            }
            return Err(error);
        }
        opened(dest, Some(self.base_branch), true)
    }
}

/// `project.set_remote` — point a project's `origin` somewhere, or unwire it.
pub struct SetRemote {
    pub repo_path: PathBuf,
    /// Empty clears the remote; removing one that is not there is not an error.
    pub url: String,
}

impl WorktreeMutation for SetRemote {
    type Output = RemoteChanged;
    fn perform(self) -> Result<Performed<Self::Output>, String> {
        let remote = match (
            self.url.is_empty(),
            git_remote_origin(&self.repo_path).is_some(),
        ) {
            (true, _) => {
                let _ = std::process::Command::new("git")
                    .arg("-C")
                    .arg(&self.repo_path)
                    .args(["remote", "remove", "origin"])
                    .output();
                None
            }
            (false, true) => {
                git_in(&self.repo_path, &["remote", "set-url", "origin", &self.url])?;
                Some(self.url)
            }
            (false, false) => {
                git_in(&self.repo_path, &["remote", "add", "origin", &self.url])?;
                Some(self.url)
            }
        };
        Ok(Performed {
            change: WorktreeChange::nothing(),
            output: RemoteChanged { remote },
        })
    }
}
