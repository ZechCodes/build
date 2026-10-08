use super::*;

/// Startup records interrupted work; it never replays Git or dispatches a reviewer.
pub fn interrupt_unfinished(store: &Store) -> Result<Vec<ReviewOpening>, String> {
    let mut openings = store
        .load_unfinished_review_openings()
        .map_err(|error| error.to_string())?;
    for opening in &mut openings {
        if opening.state != ReviewOpeningState::Preparing {
            continue;
        }
        opening.state = ReviewOpeningState::Interrupted;
        opening.error = Some(
            "Review opening was interrupted. Retry the same request or cancel its owned setup."
                .into(),
        );
        for binding in &mut opening.bindings {
            if binding.publication == ReviewPublicationState::Pending {
                binding.publication = ReviewPublicationState::Interrupted;
            }
            binding.recovery = opening.error.clone();
        }
        save(store, opening)?;
    }
    Ok(openings)
}

/// Explicitly unwind an unpublished opening. Changed refs/config remain in
/// place, and the workspace claim stays held until every owned step is undone.
pub fn cancel(
    store: &Store,
    request: &OpenReviewRequest,
    hooks: &dyn OpeningHooks,
) -> Result<ReviewOpening, String> {
    let _lock = journal::lock(request)?;
    hooks.check_workspace(&request.workspace)?;
    let mut opening = store
        .load_review_opening(&request.project_path, &request.request_id)
        .map_err(|error| error.to_string())?
        .ok_or("unknown review opening")?;
    if opening.state == ReviewOpeningState::Cancelled {
        return Ok(opening);
    }
    if opening.state == ReviewOpeningState::Published {
        return Err("published reviews cannot be unwound; close the review instead".into());
    }
    validate_opening_workspace(request, &opening)?;
    let cleanup = unwind(request, hooks, &opening);
    if let Err(error) = cleanup {
        record_failure(store, &mut opening, &error);
        return Err(error);
    }
    opening.state = ReviewOpeningState::Cancelled;
    opening.error = None;
    for binding in &mut opening.bindings {
        binding.recovery = None;
    }
    save(store, &mut opening)?;
    Ok(opening)
}

fn unwind(
    request: &OpenReviewRequest,
    hooks: &dyn OpeningHooks,
    opening: &ReviewOpening,
) -> Result<(), String> {
    // Holders and changed branches or publication refs preserve every
    // directory's setup, including snapshot pins. Preflight before any unwind.
    for binding in &opening.bindings {
        hooks.check_workspace(&request.workspace)?;
        git::validate_cleanup(&opening.task.id, &opening.request_id, binding)?;
        publication::validate_initial_cleanup(binding)?;
    }
    publication::cleanup_opening_pins(
        &opening.task.id,
        &format!("opening-{}", opening.task.id),
        &opening.bindings,
    )?;
    for binding in opening.bindings.iter().rev() {
        hooks.check_workspace(&request.workspace)?;
        // A branch with external commits blocks all further teardown for it.
        git::validate_cleanup(&opening.task.id, &opening.request_id, binding)?;
        publication::cleanup_initial(binding)?;
        publication::cleanup_remote(binding)?;
        git::cleanup_branch(&opening.task.id, &opening.request_id, binding)?;
    }
    let workspace =
        crate::workspace::restore_review_branches(&request.workspace, &opening.bindings)?;
    hooks.workspace_changed(&workspace)
}
