//! Can a copy-on-write clone be made for a project on this volume? Four checks
//! in order, first failure wins, each carrying the exact sentence a control
//! shows the user (spec §4.3). The reason is user copy, not a log line: this is
//! the one place a volume's answer is worded, and nothing here is cached — the
//! probe is three `stat`s and one tiny clone, cheap enough to ask on every use.

use std::os::unix::fs::MetadataExt;
use std::path::Path;

use super::cow::clone_tree;

const NOT_OWN_GIT_DIRECTORY: &str = "the project checkout is itself a linked worktree; clones need the repository's own .git directory";
const CROSS_VOLUME: &str =
    "the project and the worktrees folder are on different volumes; clones cannot cross volumes";

/// `Ok(())` when a clone can be made for `project` whose checkouts live under
/// `worktrees_root`, else the sentence explaining why not.
pub fn cow_availability(project: &Path, worktrees_root: &Path) -> Result<(), String> {
    if !project.join(".git").is_dir() {
        return Err(NOT_OWN_GIT_DIRECTORY.to_string());
    }
    std::fs::create_dir_all(worktrees_root).map_err(|error| error.to_string())?;
    let project_volume = std::fs::metadata(project)
        .map_err(|error| error.to_string())?
        .dev();
    let root_volume = std::fs::metadata(worktrees_root)
        .map_err(|error| error.to_string())?
        .dev();
    if project_volume != root_volume {
        return Err(CROSS_VOLUME.to_string());
    }
    clone_probe(worktrees_root)
}

/// Prove the volume clones by cloning a few bytes and removing both files. The
/// clone call is the same one `materialize` uses, so the probe cannot pass a
/// volume a real clone would fail on.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn clone_probe(worktrees_root: &Path) -> Result<(), String> {
    let source = worktrees_root.join(format!(".cow-probe-{}", std::process::id()));
    let clone = worktrees_root.join(format!(".cow-probe-{}.clone", std::process::id()));
    let result = write_then_clone(&source, &clone);
    let _ = std::fs::remove_file(&source);
    let _ = std::fs::remove_file(&clone);
    result
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn write_then_clone(source: &Path, clone: &Path) -> Result<(), String> {
    std::fs::write(source, b"cow-probe").map_err(|error| error.to_string())?;
    clone_tree(source, clone)
        .map_err(|error| format!("this volume does not support copy-on-write cloning ({error})"))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn clone_probe(_worktrees_root: &Path) -> Result<(), String> {
    Err("copy-on-write isolation is only available on macOS and Linux".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_fixture::init_repo;

    /// No platform assumption: the probe and a real single-file clone on this
    /// machine's temp volume must reach the same verdict, so the probe never
    /// promises a clone the filesystem would refuse and never refuses one it
    /// would allow.
    #[test]
    fn the_probe_agrees_with_a_real_file_clone() {
        let (dir, project) = init_repo();
        let worktrees_root = dir.path().join("worktrees");

        let probe = cow_availability(&project, &worktrees_root);

        let source = worktrees_root.join("agreement-probe");
        std::fs::write(&source, b"agreement").unwrap();
        let clone = worktrees_root.join("agreement-probe.clone");
        let real_clone = clone_tree(&source, &clone);

        assert_eq!(
            probe.is_ok(),
            real_clone.is_ok(),
            "probe said {probe:?} but a real file clone said {real_clone:?}"
        );
    }
}
