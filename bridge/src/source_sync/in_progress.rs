//! An operation part-way through on the base branch, in any worktree of the
//! repository (#268).
//!
//! A rebase detaches HEAD, so a base being rebased reads as checked out
//! nowhere; moving its ref then makes `rebase --continue` fail and strands
//! the rebased commits. A bisect of the base likewise ends by checking the
//! base out again. A merge, cherry-pick or revert keeps HEAD on its branch,
//! so it is on the base only in the worktree whose HEAD is the base.
//!
//! Only these block: Build's agent workspaces are linked worktrees of the
//! source, each on a branch of its own, and a merge stopped on a conflict in
//! one of them has nothing to do with the base.

use std::path::{Path, PathBuf};

/// What a worktree's git directory holds while an operation is part-way
/// through, what the operation is called, and the file in the same git
/// directory that names the branch it is on.
const UNFINISHED: &[(&str, &str, &str)] = &[
    ("rebase-merge", "rebase", "rebase-merge/head-name"),
    ("rebase-apply", "rebase", "rebase-apply/head-name"),
    ("rebase-apply", "git am", "HEAD"),
    ("BISECT_START", "bisect", "BISECT_START"),
    ("MERGE_HEAD", "merge", "HEAD"),
    ("CHERRY_PICK_HEAD", "cherry-pick", "HEAD"),
    ("REVERT_HEAD", "revert", "HEAD"),
    ("sequencer", "cherry-pick or revert", "HEAD"),
];

/// The first operation part-way through on `base` in any worktree of
/// `repo`'s repository, as a sentence naming it and the worktree.
pub(super) fn unfinished_on(repo: &git2::Repository, base: &str) -> Option<String> {
    worktree_git_dirs(repo)
        .into_iter()
        .find_map(|(git_dir, worktree)| {
            let (_, operation, _) = UNFINISHED.iter().find(|(marker, _, named_by)| {
                git_dir.join(marker).exists() && names_branch(&git_dir.join(named_by), base)
            })?;
            Some(format!(
                "A {operation} of {base} is in progress in {}.",
                worktree.display()
            ))
        })
}

/// Whether the file at `path` names `branch`: `ref: refs/heads/<branch>`
/// (HEAD), `refs/heads/<branch>` (a rebase's head-name) or `<branch>` (the
/// branch a bisect started from).
fn names_branch(path: &Path, branch: &str) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let named = text.trim();
    let named = named.strip_prefix("ref: ").unwrap_or(named);
    named.strip_prefix("refs/heads/").unwrap_or(named) == branch
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
