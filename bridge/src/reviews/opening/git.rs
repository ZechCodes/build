//! Exact branch placement for a recoverable review opening.

mod refs;

use crate::reviews::model::{
    ReviewBranchBinding, ReviewMembership, ReviewMembershipKind, ReviewPreparationState,
    ReviewPublicationState,
};
use crate::reviews::receivers::{canonical_common_git_dir, repository_id};
use crate::workspace::{DirectoryStatus, Workspace, WorkspaceDirectory, WorkspaceStatus};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

const OWNERSHIP_DIRECTORY: &str = "build-review-openings";
const KEEPS_BRANCH: &[u8] = b"keeps-branch";
const HISTORY_LIMIT: usize = 100_000;

/// Preview and durable planning use precisely the same read-only naming rules.
pub use plan_bindings as preview_bindings;

pub fn resolve_receiver_collision(
    binding: &mut ReviewBranchBinding,
    task_id: &str,
) -> Result<(), String> {
    resolve_receiver_collision_with(binding, task_id, |_| Ok(false))
}

/// Complete read-only planning once the service knows the persistent receiver.
/// The callback includes reservations held by other projects or workspaces.
pub fn resolve_receiver_collision_with(
    binding: &mut ReviewBranchBinding,
    task_id: &str,
    mut occupied: impl FnMut(&str) -> Result<bool, String>,
) -> Result<(), String> {
    if binding.preparation != ReviewPreparationState::Planned {
        return Err("review branch placement is already prepared".into());
    }
    let receiver = optional_receiver(binding)?;
    let suffix = format!("-{}", short_identity(task_id));
    let initial = binding.dedicated_branch_ref.clone();
    let fallback = if initial.ends_with(&suffix) {
        initial.clone()
    } else {
        format!("{initial}{suffix}")
    };
    for candidate in [initial, fallback] {
        if !occupied(&candidate)?
            && ref_available(binding, &candidate, &BTreeSet::new())?
            && receiver_ref_available(receiver.as_ref(), &candidate)?
        {
            binding.dedicated_branch_ref = candidate.clone();
            binding.receiving_ref = candidate;
            return Ok(());
        }
    }
    Err("dedicated review branch and stable task suffix are already reserved".into())
}

fn optional_receiver(binding: &ReviewBranchBinding) -> Result<Option<git2::Repository>, String> {
    match fs::symlink_metadata(&binding.receiving_repository) {
        Ok(_) => {
            crate::reviews::receivers::validate_binding_receiver(binding)?;
            git2::Repository::open_bare(&binding.receiving_repository)
                .map(Some)
                .map_err(|error| error.to_string())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn receiver_ref_available(receiver: Option<&git2::Repository>, name: &str) -> Result<bool, String> {
    match receiver {
        Some(receiver) => Ok(!ref_namespace_occupied(receiver, name)?),
        None => Ok(true),
    }
}

pub fn plan_bindings(
    workspace: &Workspace,
    memberships: &[ReviewMembership],
    base_branches: &BTreeMap<String, String>,
    task_number: u64,
    task_id: &str,
    title: &str,
) -> Result<Vec<ReviewBranchBinding>, String> {
    if !workspace.managed || workspace.status != WorkspaceStatus::Ready {
        return Err("PR opening requires a ready managed workspace".into());
    }
    if task_number == 0 || task_id.is_empty() {
        return Err("invalid review task identity".into());
    }
    let directories = participating_directories(workspace, memberships, base_branches)?;
    let mut bindings = directories
        .iter()
        .map(|directory| plan_directory(workspace, directory, base_branches))
        .collect::<Result<Vec<_>, _>>()?;
    let mut reservations = BTreeSet::new();
    for index in 0..bindings.len() {
        let shared = bindings.iter().enumerate().any(|(other_index, other)| {
            other_index != index && other.repository_id == bindings[index].repository_id
        });
        let suffix = shared.then(|| {
            format!(
                "-{}-{}",
                crate::worktree::slugify(&directories[index].name),
                short_identity(&directories[index].id)
            )
        });
        let stem = format!(
            "refs/heads/review/{task_number}-{}{}",
            crate::worktree::slugify(title),
            suffix.as_deref().unwrap_or_default()
        );
        let chosen = choose_ref(&bindings[index], &stem, task_id, &reservations)?;
        for common in binding_common_directories(&bindings[index])? {
            reservations.insert((common, chosen.clone()));
        }
        bindings[index].dedicated_branch_ref = chosen.clone();
        bindings[index].receiving_ref = chosen;
    }
    Ok(bindings)
}

fn participating_directories<'a>(
    workspace: &'a Workspace,
    memberships: &[ReviewMembership],
    base_branches: &BTreeMap<String, String>,
) -> Result<Vec<&'a WorkspaceDirectory>, String> {
    let mut directories = Vec::new();
    let mut seen = BTreeSet::new();
    for membership in memberships {
        if !seen.insert(&membership.directory_id) {
            return Err(format!(
                "duplicate review directory: {}",
                membership.directory_id
            ));
        }
        let directory = workspace
            .directories
            .iter()
            .find(|directory| directory.id == membership.directory_id)
            .ok_or_else(|| format!("unknown review directory: {}", membership.directory_id))?;
        if directory.source_id != membership.source_id {
            return Err(format!("review source identity changed: {}", directory.id));
        }
        if membership.kind == ReviewMembershipKind::Git {
            directories.push(directory);
        }
    }
    for directory_id in base_branches.keys() {
        if !directories
            .iter()
            .any(|directory| directory.id == *directory_id)
        {
            return Err(format!("unknown review base directory: {directory_id}"));
        }
    }
    Ok(directories)
}

fn plan_directory(
    workspace: &Workspace,
    directory: &WorkspaceDirectory,
    base_branches: &BTreeMap<String, String>,
) -> Result<ReviewBranchBinding, String> {
    if !directory.is_git || directory.status != DirectoryStatus::Ready {
        return Err(format!(
            "review directory is not ready Git: {}",
            directory.id
        ));
    }
    let working_repository = directory
        .path
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let root = workspace
        .root
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if !working_repository.starts_with(&root) || working_repository == root {
        return Err("review directory is outside its managed workspace".into());
    }
    let source_repository = directory
        .source_path
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let working = open_working_root(&working_repository)?;
    let source = open_source_root(&source_repository)?;
    refuse_operation(&working)?;
    let head = working
        .head()
        .and_then(|head| head.peel_to_commit())
        .map_err(|error| format!("review directory has no committed HEAD: {error}"))?
        .id();
    let original_branch_ref = current_branch_ref(&working)?;
    let base_branch_ref = base_ref(
        base_branches
            .get(&directory.id)
            .map(String::as_str)
            .unwrap_or(&directory.base_branch),
    )?;
    validate_comparison(&working, &source, head, &base_branch_ref)?;
    Ok(ReviewBranchBinding {
        directory_id: directory.id.clone(),
        source_id: directory.source_id.clone(),
        repository_id: repository_id(&source_repository)?,
        working_repository,
        source_repository: source_repository.clone(),
        initial_head: head.to_string(),
        original_branch_ref,
        dedicated_branch_ref: String::new(),
        base_branch_ref,
        receiving_repository: canonical_common_git_dir(&source_repository)?,
        receiving_ref: String::new(),
        remote_name: String::new(),
        last_received_head: None,
        preparation: ReviewPreparationState::Planned,
        publication: ReviewPublicationState::Pending,
        recovery: None,
    })
}

fn base_ref(name: &str) -> Result<String, String> {
    let name = name.strip_prefix("refs/heads/").unwrap_or(name);
    if !crate::worktree::is_ref_name(name) || name.starts_with("refs/") {
        return Err("review base must name a local source branch".into());
    }
    Ok(format!("refs/heads/{name}"))
}

fn binding_common_directories(binding: &ReviewBranchBinding) -> Result<BTreeSet<PathBuf>, String> {
    Ok(BTreeSet::from([
        canonical_common_git_dir(&binding.working_repository)?,
        canonical_common_git_dir(&binding.source_repository)?,
    ]))
}

fn choose_ref(
    binding: &ReviewBranchBinding,
    stem: &str,
    task_id: &str,
    reservations: &BTreeSet<(PathBuf, String)>,
) -> Result<String, String> {
    for candidate in [
        stem.to_string(),
        format!("{stem}-{}", short_identity(task_id)),
    ] {
        if !git2::Reference::is_valid_name(&candidate) {
            return Err("invalid dedicated review branch".into());
        }
        if ref_available(binding, &candidate, reservations)? {
            return Ok(candidate);
        }
    }
    Err(format!(
        "dedicated review branch already exists for task: {task_id}"
    ))
}

fn ref_available(
    binding: &ReviewBranchBinding,
    name: &str,
    reservations: &BTreeSet<(PathBuf, String)>,
) -> Result<bool, String> {
    for common in binding_common_directories(binding)? {
        if reservations.contains(&(common.clone(), name.to_string())) {
            return Ok(false);
        }
        let repo = git2::Repository::open_bare(common).map_err(|error| error.to_string())?;
        if ref_namespace_occupied(&repo, name)? || branch_held_elsewhere(&repo, name, None)? {
            return Ok(false);
        }
    }
    Ok(true)
}

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

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct GitDirectoryIdentity {
    path: PathBuf,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

impl GitDirectoryIdentity {
    fn read(path: &Path) -> Result<Self, String> {
        let path = path.canonicalize().map_err(|error| error.to_string())?;
        #[cfg(unix)]
        let metadata = fs::metadata(&path).map_err(|error| error.to_string())?;
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Ok(Self {
            path,
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
        })
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct BranchOwnership {
    format: u32,
    task_id: String,
    request_id: String,
    directory_id: String,
    source_id: String,
    repository_id: String,
    working_repository: PathBuf,
    source_repository: PathBuf,
    initial_head: String,
    original_branch_ref: Option<String>,
    dedicated_branch_ref: String,
    base_branch_ref: String,
    working_git_dir: GitDirectoryIdentity,
    source_git_dir: GitDirectoryIdentity,
    original_teardown: Option<Vec<u8>>,
    reflog_message: String,
}

impl BranchOwnership {
    fn new(
        task_id: &str,
        request_id: &str,
        binding: &ReviewBranchBinding,
        working: &git2::Repository,
        source: &git2::Repository,
    ) -> Result<Self, String> {
        if task_id.is_empty() || request_id.is_empty() {
            return Err("invalid review opening ownership identity".into());
        }
        Ok(Self {
            format: 1,
            task_id: task_id.into(),
            request_id: request_id.into(),
            directory_id: binding.directory_id.clone(),
            source_id: binding.source_id.clone(),
            repository_id: binding.repository_id.clone(),
            working_repository: binding.working_repository.clone(),
            source_repository: binding.source_repository.clone(),
            initial_head: binding.initial_head.clone(),
            original_branch_ref: binding.original_branch_ref.clone(),
            dedicated_branch_ref: binding.dedicated_branch_ref.clone(),
            base_branch_ref: binding.base_branch_ref.clone(),
            working_git_dir: GitDirectoryIdentity::read(working.path())?,
            source_git_dir: GitDirectoryIdentity::read(source.commondir())?,
            original_teardown: validated_original_teardown(binding, working)?,
            reflog_message: format!(
                "Build review opening {}/{}",
                short_identity(task_id),
                short_identity(request_id)
            ),
        })
    }
}

fn ownership_path(working: &git2::Repository, binding: &ReviewBranchBinding) -> PathBuf {
    working.commondir().join(OWNERSHIP_DIRECTORY).join(format!(
        "{}.json",
        short_identity(&binding.dedicated_branch_ref)
    ))
}

fn read_ownership(path: &Path) -> Result<Option<BranchOwnership>, String> {
    let parent = path.parent().ok_or("invalid review ownership path")?;
    if parent.exists() && parent.canonicalize().map_err(|error| error.to_string())? != parent {
        return Err("review ownership directory placement changed".into());
    }
    let Some(bytes) = read_regular_optional(path)? else {
        return Ok(None);
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| format!("invalid review branch ownership stamp: {error}"))
}

fn read_regular_optional(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => {
            fs::read(path).map(Some).map_err(|error| error.to_string())
        }
        Ok(_) => Err("review ownership marker is not a regular file".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn persist_ownership(path: &Path, ownership: &BranchOwnership) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or("invalid review branch ownership path")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    if parent.canonicalize().map_err(|error| error.to_string())? != parent {
        return Err("review ownership directory placement changed".into());
    }
    let bytes = serde_json::to_vec(ownership).map_err(|error| error.to_string())?;
    let temporary = parent.join(format!(".preparing-{}", uuid::Uuid::new_v4()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| error.to_string())?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| error.to_string())?;
    // Link only the complete, fsynced file into the final exclusive path. A
    // crash while writing leaves an ignored temporary stamp and no owned ref.
    let published = match fs::hard_link(&temporary, path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            match read_ownership(path)? {
                Some(existing) if existing == *ownership => Ok(()),
                _ => Err("review branch ownership stamp already belongs to another opening".into()),
            }
        }
        Err(error) => Err(error.to_string()),
    };
    let _ = fs::remove_file(&temporary);
    published?;
    sync_directory(parent)
}

fn sync_directory(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    fs::File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|error| error.to_string())?;
    let _ = path;
    Ok(())
}

fn validate_ownership(
    ownership: &BranchOwnership,
    task_id: &str,
    request_id: &str,
    binding: &ReviewBranchBinding,
    working: &git2::Repository,
    source: &git2::Repository,
) -> Result<(), String> {
    if ownership.task_id != task_id || ownership.request_id != request_id {
        return Err("review branch belongs to another opening".into());
    }
    validate_ownership_binding(ownership, binding, working, source)
}

fn validate_ownership_binding(
    ownership: &BranchOwnership,
    binding: &ReviewBranchBinding,
    working: &git2::Repository,
    source: &git2::Repository,
) -> Result<(), String> {
    let stable_fields_match = ownership.directory_id == binding.directory_id
        && ownership.source_id == binding.source_id
        && ownership.repository_id == binding.repository_id
        && ownership.working_repository == binding.working_repository
        && ownership.source_repository == binding.source_repository
        && ownership.initial_head == binding.initial_head
        && ownership.original_branch_ref == binding.original_branch_ref
        && ownership.dedicated_branch_ref == binding.dedicated_branch_ref
        && ownership.base_branch_ref == binding.base_branch_ref;
    if ownership.format != 1 || !stable_fields_match {
        return Err("review branch ownership binding changed".into());
    }
    let expected_message = format!(
        "Build review opening {}/{}",
        short_identity(&ownership.task_id),
        short_identity(&ownership.request_id)
    );
    if ownership.task_id.is_empty()
        || ownership.request_id.is_empty()
        || ownership.reflog_message != expected_message
    {
        return Err("review branch ownership operation changed".into());
    }
    if ownership.working_git_dir != GitDirectoryIdentity::read(working.path())?
        || ownership.source_git_dir != GitDirectoryIdentity::read(source.commondir())?
    {
        return Err("review repository Git directory identity changed".into());
    }
    Ok(())
}

fn validate_created_branch(
    working: &git2::Repository,
    binding: &ReviewBranchBinding,
    ownership: &BranchOwnership,
) -> Result<(), String> {
    let expected = parse_oid(&binding.initial_head)?;
    let branch = working
        .find_reference(&binding.dedicated_branch_ref)
        .map_err(|error| error.to_string())?;
    if branch.target() != Some(expected) {
        return Err("dedicated review branch advanced externally".into());
    }
    let log = working
        .reflog(&binding.dedicated_branch_ref)
        .map_err(|error| error.to_string())?;
    let entry = log
        .get(0)
        .ok_or("review branch has no owned creation reflog")?;
    if entry.id_old() != git2::Oid::zero()
        || entry.id_new() != expected
        || entry.message() != Some(ownership.reflog_message.as_str())
    {
        return Err("review branch creation ownership cannot be proved".into());
    }
    Ok(())
}

fn teardown_path(working: &git2::Repository) -> PathBuf {
    working
        .path()
        .join(crate::isolation::BRANCH_TEARDOWN_MARKER)
}

fn read_teardown(working: &git2::Repository) -> Result<Option<Vec<u8>>, String> {
    read_regular_optional(&teardown_path(working))
}

fn validated_original_teardown(
    binding: &ReviewBranchBinding,
    working: &git2::Repository,
) -> Result<Option<Vec<u8>>, String> {
    crate::isolation::branch_teardown(&binding.working_repository)
        .map_err(|error| error.to_string())?;
    read_teardown(working)
}

fn validate_teardown(
    working: &git2::Repository,
    ownership: &BranchOwnership,
) -> Result<(), String> {
    let current = read_teardown(working)?;
    if current != ownership.original_teardown && current.as_deref() != Some(KEEPS_BRANCH) {
        return Err("review branch teardown ownership changed externally".into());
    }
    Ok(())
}

fn preserve_teardown(
    working: &git2::Repository,
    ownership: &BranchOwnership,
) -> Result<(), String> {
    validate_teardown(working, ownership)?;
    write_teardown(working, Some(KEEPS_BRANCH))
}

fn restore_teardown(working: &git2::Repository, ownership: &BranchOwnership) -> Result<(), String> {
    validate_teardown(working, ownership)?;
    write_teardown(working, ownership.original_teardown.as_deref())
}

fn write_teardown(working: &git2::Repository, bytes: Option<&[u8]>) -> Result<(), String> {
    let path = teardown_path(working);
    match bytes {
        Some(bytes) => {
            let temporary = working
                .path()
                .join(format!("build-review-teardown-{}", uuid::Uuid::new_v4()));
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(|error| error.to_string())?;
            file.write_all(bytes)
                .and_then(|()| file.sync_all())
                .map_err(|error| error.to_string())?;
            fs::rename(&temporary, &path).map_err(|error| error.to_string())?;
        }
        None => match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        },
    }
    sync_directory(working.path())
}

#[cfg(test)]
mod tests;
