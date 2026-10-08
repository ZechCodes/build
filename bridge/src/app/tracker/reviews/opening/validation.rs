//! Membership and trusted workspace placement for deferred review operations.
use crate::api::v1::reviews::{ReviewOpenParams, ReviewReviewer};
use crate::app::AppState;
use crate::reviews::model::{ReviewMembership, ReviewMembershipKind};
use crate::tracker::Actor;
use crate::workspace::{Workspace, WorkspaceRegistry, WorkspaceStatus};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub(super) fn validate_actor(
    app: &AppState,
    project_id: &str,
    actor: &Actor,
    reviewer: Option<&ReviewReviewer>,
) -> Result<(), String> {
    if let Some(agent_id) = actor.agent_id() {
        app.agent_of_this_project(project_id, agent_id)?;
    }
    if let Some(ReviewReviewer::Agent { agent_id }) = reviewer {
        app.agent_of_this_project(project_id, agent_id)?;
    }
    Ok(())
}

pub(super) fn selections(
    workspace: &Workspace,
    params: &ReviewOpenParams,
) -> Result<(Vec<ReviewMembership>, BTreeMap<String, String>), String> {
    let excluded: BTreeSet<_> = params.excluded_git_directory_ids.iter().collect();
    let selected: BTreeMap<_, _> = params
        .bases
        .iter()
        .map(|base| (&base.directory_id, &base.branch))
        .collect();
    let git_ids: BTreeSet<_> = workspace
        .directories
        .iter()
        .filter(|directory| directory.is_git)
        .map(|directory| &directory.id)
        .collect();
    if !excluded.is_subset(&git_ids)
        || selected
            .keys()
            .any(|id| !git_ids.contains(id) || excluded.contains(id))
    {
        return Err(
            "invalid review params: bases and exclusions must name participating Git directories"
                .into(),
        );
    }
    let directories = workspace
        .directories
        .iter()
        .map(|directory| {
            let kind = if excluded.contains(&directory.id) {
                ReviewMembershipKind::Excluded
            } else if directory.is_git {
                ReviewMembershipKind::Git
            } else {
                ReviewMembershipKind::Live
            };
            ReviewMembership {
                directory_id: directory.id.clone(),
                source_id: directory.source_id.clone(),
                kind,
                reason: (kind == ReviewMembershipKind::Excluded)
                    .then(|| "Excluded when opening this review.".into()),
            }
        })
        .collect();
    let bases = workspace
        .directories
        .iter()
        .filter(|directory| directory.is_git && !excluded.contains(&directory.id))
        .map(|directory| {
            (
                directory.id.clone(),
                crate::isolation::local_branch_ref(
                    selected
                        .get(&directory.id)
                        .map_or(directory.base_branch.as_str(), |branch| branch.as_str()),
                ),
            )
        })
        .collect::<BTreeMap<_, _>>();
    if bases.is_empty() {
        return Err(
            "invalid review params: opening requires at least one included Git directory".into(),
        );
    }
    Ok((directories, bases))
}

pub(in crate::app) fn validate_live_workspace(
    app: &AppState,
    expected: &Workspace,
) -> Result<(), String> {
    let current = app
        .workspaces
        .get(&expected.id)
        .ok_or("conflict: review workspace no longer exists")?;
    validate_placement(current, expected)?;
    app.refuse_writers_while_reserved(&current.root)?;
    let sources = app.sources_for(&expected.project_id)?;
    for directory in &current.directories {
        app.refuse_writers_while_reserved(&directory.path)?;
        app.refuse_writers_while_reserved(&directory.source_path)?;
        let configured = sources
            .iter()
            .find(|source| source.id == directory.source_id)
            .ok_or_else(|| {
                format!(
                    "conflict: review directory {} source is no longer configured",
                    directory.id
                )
            })?;
        let unavailable = |error| {
            format!(
                "unavailable: review directory {} source path is unavailable: {error}",
                directory.id
            )
        };
        if configured.path.canonicalize().map_err(&unavailable)?
            != directory.source_path.canonicalize().map_err(unavailable)?
        {
            return Err(format!(
                "conflict: review directory {} source placement changed",
                directory.id
            ));
        }
        validate_source_reservations(app, &expected.project_id, &directory.source_id)?;
    }
    Ok(())
}

fn validate_source_reservations(
    app: &AppState,
    project_id: &str,
    source_id: &str,
) -> Result<(), String> {
    for checkout in app
        .workspaces
        .list(Some(project_id))
        .into_iter()
        .flat_map(|workspace| &workspace.directories)
        .filter(|checkout| checkout.source_id == source_id)
    {
        app.refuse_writers_while_reserved(&checkout.path)?;
    }
    Ok(())
}

pub(in crate::app) fn validate_recorded_workspace(
    registry_root: &Path,
    expected: &Workspace,
) -> Result<(), String> {
    let registry = WorkspaceRegistry::load(registry_root)?;
    let current = registry
        .get(&expected.id)
        .ok_or("conflict: review workspace no longer exists")?;
    validate_placement(current, expected)
}

fn validate_placement(current: &Workspace, expected: &Workspace) -> Result<(), String> {
    if current.id != expected.id
        || current.project_id != expected.project_id
        || current.root != expected.root
        || !current.managed
        || current.status != WorkspaceStatus::Ready
        || current.directories.len() != expected.directories.len()
    {
        return Err("conflict: review workspace identity or placement changed".into());
    }
    for directory in &expected.directories {
        let actual = current
            .directories
            .iter()
            .find(|actual| actual.id == directory.id)
            .ok_or("conflict: review workspace directory membership changed")?;
        if actual.source_id != directory.source_id
            || actual.path != directory.path
            || actual.source_path != directory.source_path
            || actual.is_git != directory.is_git
            || actual.status != directory.status
        {
            return Err(format!(
                "conflict: review directory {} placement changed",
                directory.id
            ));
        }
    }
    Ok(())
}
