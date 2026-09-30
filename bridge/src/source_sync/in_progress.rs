//! An operation part-way through in any worktree of the repository (#268).
//!
//! A rebase detaches HEAD, so a base being rebased reads as checked out
//! nowhere; moving its ref then makes `rebase --continue` fail and strands
//! the rebased commits. A merge, cherry-pick, revert or bisect likewise
//! expects the branch it started from to hold still. So before a sync moves
//! the base, every worktree's git directory is read for what git leaves
//! there mid-operation, and one found anywhere leaves the base alone.

use std::path::{Path, PathBuf};

/// What a worktree's git directory holds while an operation is part-way
/// through, and what the operation is called.
const UNFINISHED: &[(&str, &str)] = &[
    ("rebase-merge", "rebase"),
    ("rebase-apply", "rebase"),
    ("MERGE_HEAD", "merge"),
    ("CHERRY_PICK_HEAD", "cherry-pick"),
    ("REVERT_HEAD", "revert"),
    ("sequencer", "cherry-pick or revert"),
    ("BISECT_START", "bisect"),
];

/// The first operation part-way through in any worktree of `repo`'s
/// repository, as a sentence naming it and the worktree.
pub(super) fn unfinished_anywhere(repo: &git2::Repository) -> Option<String> {
    worktree_git_dirs(repo)
        .into_iter()
        .find_map(|(git_dir, worktree)| {
            let (_, operation) = UNFINISHED
                .iter()
                .find(|(marker, _)| git_dir.join(marker).exists())?;
            Some(format!(
                "A {operation} is in progress in {}.",
                worktree.display()
            ))
        })
}

/// Each worktree's git directory, with the worktree it belongs to: the main
/// one's is the common directory, a linked one's is under its `worktrees/`.
fn worktree_git_dirs(repo: &git2::Repository) -> Vec<(PathBuf, PathBuf)> {
    let common = repo.commondir().to_path_buf();
    let main = main_worktree(&common);
    let linked = repo.worktrees().map_or_else(
        |_| Vec::new(),
        |names| {
            names
                .iter()
                .flatten()
                .filter_map(|name| {
                    let worktree = repo.find_worktree(name).ok()?;
                    Some((common.join("worktrees").join(name), worktree.path().into()))
                })
                .collect()
        },
    );
    std::iter::once((common, main)).chain(linked).collect()
}

fn main_worktree(common: &Path) -> PathBuf {
    match common.file_name() {
        Some(name) if name == ".git" => common.parent().unwrap_or(common).to_path_buf(),
        _ => common.to_path_buf(),
    }
}
