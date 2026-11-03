//! Commit selections over a revision's retained Git objects.
use super::*;
use crate::api::v1::reviews::errors;
use crate::reviews::model::{ReviewBase, ReviewBaseKind};
use git2::{Oid, Repository};
use serde_json::json;

fn unavailable(_: git2::Error) -> String {
    "Source unavailable".into()
}

fn outside_range(directory: &ReviewDirectory, commit: &str) -> String {
    errors::encode(
        "invalid_params",
        "Commit is outside the review revision's range",
        json!({"reason":"commit_outside_range", "directory_id":directory.id,
            "commit":commit, "base":directory.base.as_ref().map(|base| &base.oid),
            "head":directory.head}),
    )
}

fn exact_oid(value: &str) -> Option<Oid> {
    (value.len() == 40)
        .then(|| Oid::from_str(value).ok())
        .flatten()
}

fn contains_commit(repo: &Repository, directory: &ReviewDirectory, oid: Oid) -> bool {
    let Some(head) = directory.head.as_deref().and_then(exact_oid) else {
        return false;
    };
    let Some(base) = directory.base.as_ref() else {
        return false;
    };
    if repo.find_commit(oid).is_err() || !descends_from(repo, head, oid) {
        return false;
    }
    base.kind == ReviewBaseKind::EmptyTree
        || exact_oid(&base.oid).is_some_and(|base| {
            repo.find_commit(base).is_ok()
                && base != oid
                && repo
                    .graph_descendant_of(base, oid)
                    .is_ok_and(|contained| !contained)
        })
}

fn descends_from(repo: &Repository, head: Oid, ancestor: Oid) -> bool {
    head == ancestor || repo.graph_descendant_of(head, ancestor).unwrap_or(false)
}

/// Validate a full commit OID against the saved revision's `base..head` range.
pub fn validate_commit(directory: &ReviewDirectory, commit: &str) -> Result<(), String> {
    if !directory.is_git
        || directory.status == super::super::model::ReviewDirectoryStatus::Unavailable
    {
        return Err("Source unavailable".into());
    }
    let git_dir = directory
        .common_git_dir
        .as_deref()
        .ok_or("Source unavailable")?;
    let repo = Repository::open_bare(git_dir).map_err(unavailable)?;
    if exact_oid(commit).is_some_and(|oid| contains_commit(&repo, directory, oid)) {
        Ok(())
    } else {
        Err(outside_range(directory, commit))
    }
}

pub(super) fn read_commit(
    directory: &ReviewDirectory,
    request: &ReviewReadRequest,
    repo: &Repository,
    git_dir: &Path,
) -> Result<ReviewReadResult, String> {
    let commit_oid = request
        .commit
        .as_deref()
        .ok_or("invalid review read: missing commit")?;
    let oid = exact_oid(commit_oid)
        .filter(|oid| contains_commit(repo, directory, *oid))
        .ok_or_else(|| outside_range(directory, commit_oid))?;
    let commit = repo.find_commit(oid).map_err(unavailable)?;
    let parent = commit.parent_ids().next();
    let mut selection = directory.clone();
    selection.base = Some(ReviewBase {
        kind: if parent.is_some() {
            ReviewBaseKind::Configured
        } else {
            ReviewBaseKind::EmptyTree
        },
        name: None,
        oid: parent.map_or_else(|| "empty".into(), |oid| oid.to_string()),
    });
    selection.head = Some(commit.id().to_string());
    read_changes(&selection, request, git_dir)
}

fn revision_walk<'repo>(
    repo: &'repo Repository,
    directory: &ReviewDirectory,
) -> Result<git2::Revwalk<'repo>, String> {
    let mut walk = repo.revwalk().map_err(unavailable)?;
    walk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::TIME)
        .map_err(unavailable)?;
    if let Some(head) = directory.head.as_deref() {
        walk.push(Oid::from_str(head).map_err(unavailable)?)
            .map_err(unavailable)?;
        let base = directory.base.as_ref().ok_or("Source unavailable")?;
        if base.kind != ReviewBaseKind::EmptyTree {
            walk.hide(Oid::from_str(&base.oid).map_err(unavailable)?)
                .map_err(unavailable)?;
        }
    }
    Ok(walk)
}

fn cursor_prefix(directory: &ReviewDirectory) -> String {
    format!(
        "{}:{}:{}:",
        directory.id,
        directory.base.as_ref().map_or("", |base| base.oid.as_str()),
        directory.head.as_deref().unwrap_or("")
    )
}

fn cursor_commit(
    directory: &ReviewDirectory,
    request: &ReviewReadRequest,
) -> Result<Option<Oid>, String> {
    request
        .cursor
        .as_deref()
        .map(|cursor| {
            cursor
                .strip_prefix(&cursor_prefix(directory))
                .and_then(exact_oid)
                .ok_or_else(|| "invalid review read: cursor does not match revision".into())
        })
        .transpose()
}

pub(super) fn read_commits(
    directory: &ReviewDirectory,
    request: &ReviewReadRequest,
    repo: &Repository,
) -> Result<ReviewReadResult, String> {
    let mut walk = revision_walk(repo, directory)?;
    if let Some(cursor) = cursor_commit(directory, request)? {
        let mut found = false;
        for oid in walk.by_ref() {
            if oid.map_err(unavailable)? == cursor {
                found = true;
                break;
            }
        }
        if !found {
            return Err("invalid review read: cursor commit is outside revision".into());
        }
    }
    let limit = request.limit.unwrap_or(100) as usize;
    let mut commits = Vec::new();
    for oid in walk.by_ref().take(limit) {
        commits.push(commit_row(repo, oid.map_err(unavailable)?)?);
    }
    let commits_truncated = walk.next().transpose().map_err(unavailable)?.is_some();
    let next_cursor = commits_truncated.then(|| {
        format!(
            "{}{}",
            cursor_prefix(directory),
            commits.last().expect("nonzero page limit").oid
        )
    });
    Ok(ReviewReadResult::Commits(ReviewCommits {
        commits,
        commits_truncated,
        next_cursor,
    }))
}

fn commit_row(repo: &Repository, oid: Oid) -> Result<ReviewCommit, String> {
    let commit = repo.find_commit(oid).map_err(unavailable)?;
    let tree = commit.tree().map_err(unavailable)?;
    let parent_tree = commit
        .parent_ids()
        .next()
        .map(|parent| repo.find_commit(parent)?.tree())
        .transpose()
        .map_err(unavailable)?;
    let diff = repo
        .diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), None)
        .map_err(unavailable)?;
    let stats = diff.stats().map_err(unavailable)?;
    let author = commit.author();
    let author_name = crate::gitgui::truncate_at_utf8_boundary(
        author.name().unwrap_or("").to_owned(),
        crate::gitgui::GIT_SUBJECT_MAX_BYTES,
    )
    .0;
    let subject = crate::gitgui::truncate_at_utf8_boundary(
        commit.summary().unwrap_or("").to_owned(),
        crate::gitgui::GIT_SUBJECT_MAX_BYTES,
    )
    .0;
    let time = author.when().seconds();
    Ok(ReviewCommit {
        oid: oid.to_string(),
        parents: commit
            .parent_ids()
            .map(|parent| parent.to_string())
            .collect(),
        subject,
        author_name,
        time,
        insertions: stats.insertions() as u64,
        deletions: stats.deletions() as u64,
        files_changed: stats.files_changed() as u64,
    })
}

/// Compare an earlier revision's head tree with this revision's head tree.
/// If the earlier object cannot be read, use the current revision's recorded
/// comparison base and explicitly identify that fallback in the response.
pub fn read_since(
    directory: &ReviewDirectory,
    earlier: &ReviewDirectory,
    request: &ReviewReadRequest,
) -> Result<ReviewReadResult, String> {
    validate_request(request)?;
    if !directory.is_git
        || directory.status == super::super::model::ReviewDirectoryStatus::Unavailable
    {
        return Err("Source unavailable".into());
    }
    if request.mode != ReviewReadMode::Since {
        return Err("invalid review read: expected since mode".into());
    }
    let git_dir = directory
        .common_git_dir
        .as_deref()
        .ok_or("Source unavailable")?;
    let repo = Repository::open_bare(git_dir).map_err(unavailable)?;
    let previous = earlier.head.as_deref().and_then(exact_oid);
    let readable = previous.is_some_and(|oid| {
        repo.find_commit(oid)
            .and_then(|commit| commit.tree())
            .is_ok()
    });
    let unborn = earlier.is_git
        && earlier.status == super::super::model::ReviewDirectoryStatus::NoCommits
        && earlier.head.is_none();
    let fallback = !readable && !unborn;
    let rewritten = fallback
        || (readable
            && !previous.is_some_and(|oid| {
                directory
                    .head
                    .as_deref()
                    .and_then(exact_oid)
                    .is_some_and(|head| descends_from(&repo, head, oid))
            }));
    let mut selection = directory.clone();
    if !fallback {
        selection.base = Some(ReviewBase {
            kind: if readable {
                ReviewBaseKind::Configured
            } else {
                ReviewBaseKind::EmptyTree
            },
            name: None,
            oid: earlier.head.clone().unwrap_or_else(|| "empty".into()),
        });
    }
    let mut result = read_changes(&selection, request, git_dir)?;
    if let ReviewReadResult::Changes(changes) = &mut result {
        changes.rewritten = rewritten;
        changes.since_baseline = Some(if fallback {
            SinceBaseline::Base
        } else {
            SinceBaseline::PreviousHead
        });
    }
    Ok(result)
}
