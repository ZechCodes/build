//! Exact branch placement for a recoverable review opening.

mod ownership;
mod planning;
mod refs;

use ownership::{
    ownership_path, persist_ownership, preserve_teardown, read_ownership, restore_teardown,
    sync_directory, validate_created_branch, validate_ownership, validate_ownership_binding,
    validate_teardown, BranchOwnership,
};
pub use planning::{
    plan_bindings, preview_bindings, resolve_receiver_collision, resolve_receiver_collision_with,
};

use crate::reviews::model::{ReviewBranchBinding, ReviewPreparationState};
use crate::reviews::receivers::repository_id;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

const HISTORY_LIMIT: usize = 100_000;

fn ref_exists(repo: &git2::Repository, name: &str) -> Result<bool, String> {
    match repo.find_reference(name) {
        Ok(_) => Ok(true),
        Err(error) if error.code() == git2::ErrorCode::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

fn ref_namespace_occupied(repo: &git2::Repository, name: &str) -> Result<bool, String> {
    let mut prefix = name;
    while prefix.starts_with("refs/heads/") {
        if ref_exists(repo, prefix)? {
            return Ok(true);
        }
        let Some((parent, _)) = prefix.rsplit_once('/') else {
            break;
        };
        prefix = parent;
    }
    let mut children = repo
        .references_glob(&format!("{name}/*"))
        .map_err(|error| error.to_string())?;
    match children.next() {
        Some(reference) => reference.map(|_| true).map_err(|error| error.to_string()),
        None => Ok(false),
    }
}

pub fn prepare_branch(
    task_id: &str,
    request_id: &str,
    binding: &mut ReviewBranchBinding,
) -> Result<(), String> {
    let (working, source) = validate_repositories(binding)?;
    refs::recover(&working)?;
    validate_binding_comparison(binding, &working, &source)?;
    let marker = ownership_path(&working, binding);
    let ownership = match read_ownership(&marker)? {
        Some(ownership) => {
            validate_ownership(&ownership, task_id, request_id, binding, &working, &source)?;
            ownership
        }
        None => {
            validate_head(&working, binding, false)?;
            refuse_occupied_branch(&working, binding)?;
            let ownership = BranchOwnership::new(task_id, request_id, binding, &working, &source)?;
            persist_ownership(&marker, &ownership)?;
            ownership
        }
    };
    validate_teardown(&working, &ownership)?;
    ensure_branch(&working, binding, &ownership)?;
    let mut locks = refs::ReferenceLocks::acquire(&working, binding)?;
    validate_created_branch(&working, binding, &ownership)?;
    validate_original_ref(&working, binding)?;
    validate_head_original_or_dedicated(&working, binding)?;
    refuse_operation(&working)?;
    refuse_other_holder(&working, binding)?;
    preserve_teardown(&working, &ownership)?;
    // Point HEAD at the new ref without a checkout: the index and every file
    // retain their bytes, and the original work branch remains intact. HEAD
    // itself stays locked from the validation through this symbolic update.
    locks.set_head(binding, false)?;
    binding.preparation = ReviewPreparationState::BranchCreated;
    Ok(())
}

fn ensure_branch(
    working: &git2::Repository,
    binding: &ReviewBranchBinding,
    ownership: &BranchOwnership,
) -> Result<(), String> {
    let mut locks = refs::ReferenceLocks::acquire(working, binding)?;
    if ref_exists(working, &binding.dedicated_branch_ref)? {
        return validate_created_branch(working, binding, ownership);
    }
    validate_head(working, binding, false)?;
    validate_original_ref(working, binding)?;
    refuse_operation(working)?;
    refuse_other_holder(working, binding)?;
    locks.create_branch(binding, &ownership.reflog_message)
}

pub fn cleanup_branch(
    task_id: &str,
    request_id: &str,
    binding: &ReviewBranchBinding,
) -> Result<(), String> {
    validate_cleanup(task_id, request_id, binding)?;
    let (working, source) = validate_repositories(binding)?;
    let marker = ownership_path(&working, binding);
    let Some(ownership) = read_ownership(&marker)? else {
        return Ok(());
    };
    validate_ownership(&ownership, task_id, request_id, binding, &working, &source)?;
    let mut locks = refs::ReferenceLocks::acquire(&working, binding)?;
    validate_cleanup_state(&working, binding, &ownership)?;
    let packed = locks.prepare_removal(binding)?;
    if current_branch_ref(&working)?.as_deref() == Some(&binding.dedicated_branch_ref) {
        refuse_original_holder(&working, binding)?;
        locks.set_head(binding, true)?;
    }
    locks.remove_branch(binding, packed.as_deref())?;
    restore_teardown(&working, &ownership)?;
    fs::remove_file(marker).map_err(|error| error.to_string())
}

/// Validate all branch cleanup conditions before aliases or receiver refs are
/// removed. Cancellation still checks them again while holding ref locks.
pub fn validate_cleanup(
    task_id: &str,
    request_id: &str,
    binding: &ReviewBranchBinding,
) -> Result<(), String> {
    let (working, source) = validate_repositories(binding)?;
    refs::recover(&working)?;
    let Some(ownership) = read_ownership(&ownership_path(&working, binding))? else {
        if ref_exists(&working, &binding.dedicated_branch_ref)? {
            return Err("review branch has no opening ownership stamp".into());
        }
        return validate_head(&working, binding, false);
    };
    validate_ownership(&ownership, task_id, request_id, binding, &working, &source)?;
    let locks = refs::ReferenceLocks::acquire(&working, binding)?;
    locks.prepare_removal(binding)?;
    validate_cleanup_state(&working, binding, &ownership)
}

fn validate_cleanup_state(
    working: &git2::Repository,
    binding: &ReviewBranchBinding,
    ownership: &BranchOwnership,
) -> Result<(), String> {
    if ref_exists(working, &binding.dedicated_branch_ref)? {
        validate_created_branch(working, binding, ownership)?;
    }
    validate_original_ref(working, binding)?;
    validate_head_original_or_dedicated(working, binding)?;
    refuse_operation(working)?;
    refuse_other_holder(working, binding)?;
    refuse_original_holder(working, binding)?;
    validate_teardown(working, ownership)
}

pub fn validate_current(
    binding: &ReviewBranchBinding,
    expected: ReviewPreparationState,
) -> Result<(), String> {
    let (working, source) = validate_repositories(binding)?;
    refs::recover(&working)?;
    validate_binding_comparison(binding, &working, &source)?;
    refuse_operation(&working)?;
    validate_original_ref(&working, binding)?;
    let prepared = expected != ReviewPreparationState::Planned;
    validate_head(&working, binding, prepared)?;
    refuse_other_holder(&working, binding)?;
    if prepared {
        let ownership = read_ownership(&ownership_path(&working, binding))?
            .ok_or("review branch has no opening ownership stamp")?;
        validate_ownership_binding(&ownership, binding, &working, &source)?;
        validate_created_branch(&working, binding, &ownership)?;
        validate_teardown(&working, &ownership)?;
    } else {
        refuse_occupied_branch(&working, binding)?;
    }
    Ok(())
}

fn validate_repositories(
    binding: &ReviewBranchBinding,
) -> Result<(git2::Repository, git2::Repository), String> {
    validate_ref(&binding.dedicated_branch_ref, "refs/heads/review/")?;
    validate_ref(&binding.base_branch_ref, "refs/heads/")?;
    if let Some(original) = &binding.original_branch_ref {
        validate_ref(original, "refs/heads/")?;
    }
    let working = open_working_root(&binding.working_repository)?;
    let source = open_source_root(&binding.source_repository)?;
    if repository_id(&binding.source_repository)? != binding.repository_id {
        return Err("review source repository identity changed".into());
    }
    Ok((working, source))
}

fn validate_binding_comparison(
    binding: &ReviewBranchBinding,
    working: &git2::Repository,
    source: &git2::Repository,
) -> Result<(), String> {
    validate_comparison(
        working,
        source,
        parse_oid(&binding.initial_head)?,
        &binding.base_branch_ref,
    )
}

fn validate_ref(name: &str, prefix: &str) -> Result<(), String> {
    if !name.starts_with(prefix) || !git2::Reference::is_valid_name(name) {
        return Err("invalid review branch ref".into());
    }
    Ok(())
}

fn open_working_root(path: &Path) -> Result<git2::Repository, String> {
    let repo = open_source_root(path)?;
    if repo.is_bare() {
        return Err("review working repository is bare".into());
    }
    Ok(repo)
}

fn open_source_root(path: &Path) -> Result<git2::Repository, String> {
    let canonical = path.canonicalize().map_err(|error| error.to_string())?;
    if canonical != path {
        return Err("review repository placement changed".into());
    }
    let repo = git2::Repository::open(path).map_err(|error| error.to_string())?;
    let root = repo
        .workdir()
        .unwrap_or(repo.path())
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if root != canonical {
        return Err("review directory is not the repository root".into());
    }
    Ok(repo)
}

fn refuse_operation(repo: &git2::Repository) -> Result<(), String> {
    if repo.state() != git2::RepositoryState::Clean {
        return Err("review repository has a Git operation in progress".into());
    }
    if [
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "rebase-merge",
        "rebase-apply",
        "sequencer",
        "BISECT_LOG",
    ]
    .iter()
    .any(|name| repo.path().join(name).exists())
    {
        return Err("review repository has a Git operation in progress".into());
    }
    Ok(())
}

fn current_branch_ref(repo: &git2::Repository) -> Result<Option<String>, String> {
    Ok(repo
        .find_reference("HEAD")
        .map_err(|error| error.to_string())?
        .symbolic_target()
        .map(str::to_owned))
}

fn validate_head(
    repo: &git2::Repository,
    binding: &ReviewBranchBinding,
    prepared: bool,
) -> Result<(), String> {
    let expected = if prepared {
        Some(binding.dedicated_branch_ref.as_str())
    } else {
        binding.original_branch_ref.as_deref()
    };
    let head = repo
        .head()
        .and_then(|reference| reference.peel_to_commit())
        .map_err(|error| error.to_string())?
        .id();
    if current_branch_ref(repo)?.as_deref() != expected || head != parse_oid(&binding.initial_head)?
    {
        return Err("review working HEAD changed after planning".into());
    }
    Ok(())
}

fn validate_head_original_or_dedicated(
    repo: &git2::Repository,
    binding: &ReviewBranchBinding,
) -> Result<(), String> {
    if current_branch_ref(repo)?.as_deref() == Some(&binding.dedicated_branch_ref) {
        validate_head(repo, binding, true)
    } else {
        validate_head(repo, binding, false)
    }
}

fn validate_original_ref(
    repo: &git2::Repository,
    binding: &ReviewBranchBinding,
) -> Result<(), String> {
    if let Some(original) = &binding.original_branch_ref {
        if repo
            .find_reference(original)
            .map_err(|error| error.to_string())?
            .target()
            != Some(parse_oid(&binding.initial_head)?)
        {
            return Err("original review work branch changed".into());
        }
    }
    Ok(())
}

fn refuse_occupied_branch(
    repo: &git2::Repository,
    binding: &ReviewBranchBinding,
) -> Result<(), String> {
    if ref_exists(repo, &binding.dedicated_branch_ref)? {
        return Err("dedicated review branch already exists without ownership".into());
    }
    refuse_other_holder(repo, binding)
}

fn refuse_other_holder(
    repo: &git2::Repository,
    binding: &ReviewBranchBinding,
) -> Result<(), String> {
    if branch_held_elsewhere(repo, &binding.dedicated_branch_ref, Some(repo.path()))? {
        return Err("dedicated review branch is checked out elsewhere".into());
    }
    Ok(())
}

fn branch_held_elsewhere(
    repo: &git2::Repository,
    branch: &str,
    allowed_git_dir: Option<&Path>,
) -> Result<bool, String> {
    find_branch_holder(repo, branch, allowed_git_dir).map(|holder| holder.is_some())
}

fn refuse_original_holder(
    repo: &git2::Repository,
    binding: &ReviewBranchBinding,
) -> Result<(), String> {
    let Some(original) = &binding.original_branch_ref else {
        return Ok(());
    };
    if let Some(holder) = find_branch_holder(repo, original, Some(repo.path()))? {
        return Err(format!(
            "original review work branch {original} is in use at {}; release that worktree before cancelling",
            holder.display()
        ));
    }
    Ok(())
}

fn find_branch_holder(
    repo: &git2::Repository,
    branch: &str,
    allowed_git_dir: Option<&Path>,
) -> Result<Option<PathBuf>, String> {
    let common = repo
        .commondir()
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let allowed = allowed_git_dir
        .map(Path::canonicalize)
        .transpose()
        .map_err(|error| error.to_string())?;
    let mut git_dirs = vec![common.clone()];
    match fs::read_dir(common.join("worktrees")) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry.map_err(|error| error.to_string())?;
                if entry
                    .file_type()
                    .map_err(|error| error.to_string())?
                    .is_dir()
                {
                    git_dirs.push(entry.path());
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    for git_dir in git_dirs {
        if allowed.as_ref() == Some(&git_dir) {
            continue;
        }
        if git_directory_holds_branch(&git_dir, branch)? {
            return Ok(Some(git_dir));
        }
    }
    Ok(None)
}

fn git_directory_holds_branch(git_dir: &Path, branch: &str) -> Result<bool, String> {
    let head = fs::read_to_string(git_dir.join("HEAD")).map_err(|error| error.to_string())?;
    if head.trim().strip_prefix("ref: ") == Some(branch) {
        return Ok(true);
    }
    let short_branch = branch.strip_prefix("refs/heads/").unwrap_or(branch);
    for marker in [
        "rebase-merge/head-name",
        "rebase-apply/head-name",
        "BISECT_START",
    ] {
        if let Some(marked) = read_optional_git_marker(&git_dir.join(marker))? {
            if marked.trim() == branch || marked.trim() == short_branch {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn read_optional_git_marker(path: &Path) -> Result<Option<String>, String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn validate_comparison(
    working: &git2::Repository,
    source: &git2::Repository,
    head: git2::Oid,
    base: &str,
) -> Result<(), String> {
    working
        .find_commit(head)
        .map_err(|error| error.to_string())?;
    let base = source
        .find_reference(base)
        .and_then(|reference| reference.peel_to_commit())
        .map_err(|error| format!("source review base unavailable: {error}"))?
        .id();
    // A clone can have new work absent from its source while its source base
    // has advanced beyond the clone. Walk both stores without importing refs,
    // objects, or an index merely to decide whether their histories relate.
    let mut work_seen = HashSet::new();
    let mut base_seen = HashSet::new();
    let mut work_pending = std::collections::VecDeque::from([head]);
    let mut base_pending = std::collections::VecDeque::from([base]);
    while !work_pending.is_empty() || !base_pending.is_empty() {
        if advance_history(
            working,
            source,
            &mut work_seen,
            &base_seen,
            &mut work_pending,
        )? || advance_history(
            source,
            working,
            &mut base_seen,
            &work_seen,
            &mut base_pending,
        )? {
            return Ok(());
        }
    }
    Err("review HEAD and source base have unrelated histories".into())
}

fn advance_history(
    first: &git2::Repository,
    second: &git2::Repository,
    visited: &mut HashSet<git2::Oid>,
    other: &HashSet<git2::Oid>,
    pending: &mut std::collections::VecDeque<git2::Oid>,
) -> Result<bool, String> {
    let Some(oid) = pending.pop_front() else {
        return Ok(false);
    };
    if other.contains(&oid) {
        return Ok(true);
    }
    if !visited.insert(oid) {
        return Ok(false);
    }
    if visited.len() > HISTORY_LIMIT {
        return Err("review history comparison exceeded its budget".into());
    }
    let commit = first
        .find_commit(oid)
        .or_else(|_| second.find_commit(oid))
        .map_err(|error| error.to_string())?;
    for parent in commit.parent_ids() {
        if !visited.contains(&parent) {
            pending.push_back(parent);
        }
    }
    Ok(false)
}

fn parse_oid(value: &str) -> Result<git2::Oid, String> {
    git2::Oid::from_str(value).map_err(|error| error.to_string())
}

fn short_identity(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))[..12].to_string()
}

#[cfg(test)]
mod tests;
