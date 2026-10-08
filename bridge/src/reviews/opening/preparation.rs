use super::*;

pub(super) fn validate_request(request: &OpenReviewRequest) -> Result<(), String> {
    if !request.workspace.managed {
        return Err("Open review requires a managed workspace. Create a managed workspace from this checkout first.".into());
    }
    if request.workspace.id != request.request.workspace_id
        || request.workspace.status != crate::workspace::WorkspaceStatus::Ready
    {
        return Err("review opening needs the selected ready workspace".into());
    }
    let expected: std::collections::BTreeSet<_> = request
        .workspace
        .directories
        .iter()
        .map(|directory| (&directory.id, &directory.source_id))
        .collect();
    let found: std::collections::BTreeSet<_> = request
        .request
        .directories
        .iter()
        .map(|member| (&member.directory_id, &member.source_id))
        .collect();
    if expected != found || found.len() != request.request.directories.len() {
        return Err("review membership must explicitly include, exclude or mark live every workspace directory".into());
    }
    for member in &request.request.directories {
        let directory = request
            .workspace
            .directories
            .iter()
            .find(|directory| directory.id == member.directory_id)
            .expect("membership checked");
        if (member.kind == crate::reviews::model::ReviewMembershipKind::Live && directory.is_git)
            || (member.kind == crate::reviews::model::ReviewMembershipKind::Git
                && !directory.is_git)
        {
            return Err(format!("invalid review membership for {}", directory.id));
        }
        if member.kind == crate::reviews::model::ReviewMembershipKind::Excluded
            && member
                .reason
                .as_deref()
                .is_none_or(|reason| reason.trim().is_empty())
        {
            return Err(format!(
                "excluded directory {} needs a visible reason",
                directory.id
            ));
        }
    }
    Ok(())
}

pub(super) fn prepare(
    store: &Store,
    request: &OpenReviewRequest,
    hooks: &dyn OpeningHooks,
    opening: &mut ReviewOpening,
) -> Result<Review, String> {
    hooks.checkpoint(OpeningStep::Reserved)?;
    opening.state = ReviewOpeningState::Preparing;
    opening.error = None;
    if opening.bindings.is_empty() {
        opening.bindings = preview_branches(store, request, opening.task.number, &opening.task.id)?;
    }
    save(store, opening)?;
    hooks.checkpoint(OpeningStep::Planned)?;
    for index in 0..opening.bindings.len() {
        prepare_directory(store, request, hooks, opening, index)?;
    }
    let snapshot = capture_initial(request, opening)?;
    hooks.checkpoint(OpeningStep::SnapshotPinned)?;
    hooks.check_workspace(&request.workspace)?;
    let workspace =
        crate::workspace::publish_review_branches(&request.workspace, &opening.bindings)?;
    hooks.workspace_changed(&workspace)?;
    hooks.checkpoint(OpeningStep::WorkspaceRecorded)?;
    initialize_dispatch(request, opening)?;
    let event = TaskEvent::new(
        &opening.task.id,
        opening.request.creator.clone(),
        TaskEventKind::Created,
        serde_json::json!({"title": opening.task.title, "review": {"mode": "pull_request", "snapshot_id": snapshot.id, "cause": "opened"}}),
        &opening.created_at,
    );
    publication::with_initial_receivers_locked(
        &opening.bindings,
        &opening.task.id,
        &snapshot.id,
        || {
            store
                .publish_review_opening(
                    &opening.project_path,
                    &opening.request_id,
                    opening.version,
                    snapshot.clone(),
                    &[event],
                )
                .map_err(|error| error.to_string())
        },
    )
}

/// Read-only preview uses the same repository, receiver and durable reservation
/// collision checks as creation. A task number hint reserves no task identity.
pub fn preview_branches(
    store: &Store,
    request: &OpenReviewRequest,
    task_number: u64,
    task_id: &str,
) -> Result<Vec<crate::reviews::model::ReviewBranchBinding>, String> {
    validate_request(request)?;
    let mut bindings = git::plan_bindings(
        &request.workspace,
        &request.request.directories,
        &request.request.base_branches,
        task_number,
        task_id,
        &request.request.title,
    )?;
    let mut reserved_remotes =
        std::collections::BTreeMap::<PathBuf, std::collections::BTreeSet<String>>::new();
    for binding in &mut bindings {
        let receiver =
            receivers::plan_receiver(&binding.source_repository, &request.receiver_root)?;
        binding.repository_id = receiver.repository_id;
        binding.receiving_repository = receiver.path;
        let repository_id = binding.repository_id.clone();
        git::resolve_receiver_collision_with(binding, task_id, |candidate| {
            store
                .review_branch_owner(&repository_id, candidate)
                .map(|owner| owner.is_some_and(|owner| owner != task_id))
                .map_err(|error| error.to_string())
        })?;
        let common_dir = git2::Repository::open(&binding.working_repository)
            .map_err(|error| error.to_string())?
            .commondir()
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let reserved = reserved_remotes.entry(common_dir).or_default();
        binding.remote_name = publication::choose_remote_name_avoiding(binding, reserved)?;
        reserved.insert(binding.remote_name.clone());
    }
    Ok(bindings)
}

fn prepare_directory(
    store: &Store,
    request: &OpenReviewRequest,
    hooks: &dyn OpeningHooks,
    opening: &mut ReviewOpening,
    index: usize,
) -> Result<(), String> {
    hooks.check_workspace(&request.workspace)?;
    let binding = &opening.bindings[index];
    let (_source_lock, _) = crate::source_sync::SyncLock::acquire(
        &binding.source_repository,
        std::time::Duration::from_secs(30),
    )
    .ok_or("busy: source repository is being synchronized")?;
    let receiver = receivers::plan_receiver(&binding.source_repository, &request.receiver_root)?;
    if receiver.path != binding.receiving_repository
        || receiver.repository_id != binding.repository_id
    {
        return Err(
            "review receiver identity changed; restore its original placement before retrying"
                .into(),
        );
    }
    receivers::ensure_receiver(&receiver)?;
    hooks.checkpoint(OpeningStep::ReceiverCreated)?;
    publication::validate_bases(std::slice::from_ref(binding))?;
    git::prepare_branch(
        &opening.task.id,
        &opening.request_id,
        &mut opening.bindings[index],
    )?;
    hooks.checkpoint(OpeningStep::BranchCreated)?;
    save(store, opening)?;
    hooks.check_workspace(&request.workspace)?;
    git::validate_current(
        &opening.bindings[index],
        ReviewPreparationState::BranchCreated,
    )?;
    publication::configure_remote(&opening.bindings[index])?;
    opening.bindings[index].preparation = ReviewPreparationState::RemoteConfigured;
    hooks.checkpoint(OpeningStep::RemoteConfigured)?;
    save(store, opening)?;
    hooks.check_workspace(&request.workspace)?;
    git::validate_current(
        &opening.bindings[index],
        ReviewPreparationState::RemoteConfigured,
    )?;
    let received = publication::publish_initial(&opening.bindings[index])
        .map_err(|error| error.to_string())?;
    opening.bindings[index].last_received_head = Some(received);
    opening.bindings[index].preparation = ReviewPreparationState::Ready;
    opening.bindings[index].publication = ReviewPublicationState::Published;
    opening.bindings[index].recovery = None;
    hooks.checkpoint(OpeningStep::RefReceived)?;
    save(store, opening)
}

fn capture_initial(
    request: &OpenReviewRequest,
    opening: &ReviewOpening,
) -> Result<ReviewSnapshot, String> {
    publication::capture_snapshot(
        &opening.task.id,
        &format!("opening-{}", opening.task.id),
        &request.workspace,
        &opening.bindings,
        &opening.request.directories,
        &opening.request.creator,
    )
}
