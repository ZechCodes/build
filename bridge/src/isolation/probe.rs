//! Can a copy-on-write clone be made for a project on this volume? Four checks
//! in order, first failure wins, each carrying the exact sentence a control
//! shows the user (spec §4.3). The reason is user copy, not a log line: this is
//! the one place a volume's answer is worded, and nothing here is cached — the
//! probe is three `stat`s and one tiny clone, cheap enough to ask on every use.

use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::atomic::AtomicU64;

use super::cow::clone_tree;

const NOT_OWN_GIT_DIRECTORY: &str = "the project checkout is itself a linked worktree; clones need the repository's own .git directory";
const CROSS_VOLUME: &str =
    "the project and the worktrees folder are on different volumes; clones cannot cross volumes";
const WORKTREES_ROOT_UNREADABLE: &str = "the worktrees folder cannot be created or read";
const PROJECT_UNREADABLE: &str = "the project folder cannot be read";

/// A sentence this module owns with the operating system's words in
/// parentheses, so nothing the probe answers is raw io text: every `Err` here
/// is the copy a control shows.
fn because(sentence: &str, error: std::io::Error) -> String {
    format!("{sentence} ({error})")
}

/// `Ok(())` when a clone can be made for `project` whose checkouts live under
/// `worktrees_root`, else the sentence explaining why not.
pub fn cow_availability(project: &Path, worktrees_root: &Path) -> Result<(), String> {
    if !project.join(".git").is_dir() {
        return Err(NOT_OWN_GIT_DIRECTORY.to_string());
    }
    std::fs::create_dir_all(worktrees_root)
        .map_err(|error| because(WORKTREES_ROOT_UNREADABLE, error))?;
    let project_volume = std::fs::metadata(project)
        .map_err(|error| because(PROJECT_UNREADABLE, error))?
        .dev();
    let root_volume = std::fs::metadata(worktrees_root)
        .map_err(|error| because(WORKTREES_ROOT_UNREADABLE, error))?
        .dev();
    if project_volume != root_volume {
        return Err(CROSS_VOLUME.to_string());
    }
    clone_probe(worktrees_root)
}

/// How many probes this process has run, so two running at once cannot name
/// the same scratch file and delete each other's source mid-clone.
#[cfg(any(target_os = "macos", target_os = "linux"))]
static PROBES_RUN: AtomicU64 = AtomicU64::new(0);

/// Prove the volume clones by cloning a few bytes and removing both files. The
/// clone call is the same one `materialize` uses, so the probe cannot pass a
/// volume a real clone would fail on.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn clone_probe(worktrees_root: &Path) -> Result<(), String> {
    let scratch = format!(
        ".cow-probe-{}-{}",
        std::process::id(),
        PROBES_RUN.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let source = worktrees_root.join(&scratch);
    let clone = worktrees_root.join(format!("{scratch}.clone"));
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

/// Whether a clone can be made under `dir` on this machine. On failure it
/// prints the reason and answers false, so a clone test says aloud why it did
/// nothing rather than passing without exercising anything. Every clone test —
/// the backend's and the façade's — opens with it, so the skip rule and the
/// project it probes with are one fact.
#[cfg(test)]
pub(crate) fn cow_or_skip(dir: &Path) -> bool {
    let project = dir.join("cow-probe-project");
    std::fs::create_dir_all(project.join(".git")).unwrap();
    match cow_availability(&project, &dir.join("cow-probe-worktrees")) {
        Ok(()) => true,
        Err(reason) => {
            eprintln!("skipping: {reason}");
            false
        }
    }
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

    /// Every sentence the probe returns is one it wrote. A worktrees folder
    /// that cannot be made is a volume answer like any other, so it reads as
    /// copy the controls can show, with the operating system's words in
    /// parentheses rather than standing alone.
    #[test]
    fn an_unmakeable_worktrees_folder_answers_in_the_probes_own_words() {
        let (dir, project) = init_repo();
        let blocking_file = dir.path().join("not-a-folder");
        std::fs::write(&blocking_file, b"in the way").unwrap();

        let reason = cow_availability(&project, &blocking_file.join("worktrees")).unwrap_err();

        assert!(reason.starts_with(WORKTREES_ROOT_UNREADABLE), "{reason}");
        assert!(
            reason.ends_with(')'),
            "the os error is not parenthetical: {reason}"
        );
    }

    /// Probes running at the same time in one process must not see each
    /// other's scratch files: a shared name lets one probe delete another's
    /// source mid-clone and report a volume that clones fine as one that
    /// cannot. The bridge probes on every create and on every settings read,
    /// all of which it serves concurrently.
    #[test]
    fn concurrent_probes_do_not_disturb_each_other() {
        let (dir, project) = init_repo();
        if !cow_or_skip(dir.path()) {
            return;
        }
        let worktrees_root = dir.path().join("worktrees");

        let failures: Vec<String> = std::thread::scope(|scope| {
            let probes: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        (0..25)
                            .filter_map(|_| cow_availability(&project, &worktrees_root).err())
                            .collect::<Vec<String>>()
                    })
                })
                .collect();
            probes
                .into_iter()
                .flat_map(|probe| probe.join().unwrap())
                .collect()
        });

        assert!(
            failures.is_empty(),
            "concurrent probes refused a volume that clones: {failures:?}"
        );
    }
}
