//! Read-only branch naming and binding planning for review openings.

use super::{
    branch_held_elsewhere, current_branch_ref, open_source_root, open_working_root,
    ref_namespace_occupied, refuse_operation, short_identity, validate_comparison,
};
use crate::reviews::model::{
    ReviewBranchBinding, ReviewMembership, ReviewMembershipKind, ReviewPreparationState,
    ReviewPublicationState,
};
use crate::reviews::receivers::{canonical_common_git_dir, repository_id};
use crate::workspace::{DirectoryStatus, Workspace, WorkspaceDirectory, WorkspaceStatus};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

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
