use super::*;

pub(super) fn initialize_dispatch(
    request: &OpenReviewRequest,
    opening: &ReviewOpening,
) -> Result<(), String> {
    let path = journal::path(&request.receiver_root, &opening.task.id, "dispatch");
    if journal::read::<ReviewerDispatch>(&path)?.is_none() {
        let status = if opening.request.reviewer.is_some() {
            ReviewerDispatch::Pending
        } else {
            ReviewerDispatch::NotRequested
        };
        journal::write(&path, &status)?;
    }
    Ok(())
}

pub(super) fn deliver_reviewer(
    store: &Store,
    request: &OpenReviewRequest,
    hooks: &dyn OpeningHooks,
    task_id: &str,
    retry: bool,
) -> Result<(), String> {
    let status = journal::dispatch(&request.receiver_root, task_id)?;
    if status != ReviewerDispatch::Pending
        && !(retry && matches!(status, ReviewerDispatch::Failed { .. }))
    {
        return Ok(());
    }
    let task = store
        .load_tracker_task(task_id)
        .map_err(|error| error.to_string())?
        .ok_or("review task is unavailable for dispatch")?;
    let status = match hooks.dispatch_reviewer(&task, &format!("review-open-{}", task.id)) {
        Ok(()) => ReviewerDispatch::Delivered,
        Err(error) => ReviewerDispatch::Failed { error },
    };
    journal::write(
        &journal::path(&request.receiver_root, task_id, "dispatch"),
        &status,
    )
}

/// Retry delivery only; never repeat branch creation or publication.
pub fn retry_reviewer_dispatch(
    store: &Store,
    request: &OpenReviewRequest,
    hooks: &dyn OpeningHooks,
) -> Result<OpenedReview, String> {
    let _lock = journal::lock(request)?;
    let opening = store
        .load_review_opening(&request.project_path, &request.request_id)
        .map_err(|error| error.to_string())?
        .ok_or("unknown review opening")?;
    if opening.state != ReviewOpeningState::Published {
        return Err("review must be published before dispatch".into());
    }
    deliver_reviewer(store, request, hooks, &opening.task.id, true)?;
    result(store, request, &opening.task.id)
}

pub fn reviewer_dispatch(
    receiver_root: &std::path::Path,
    task_id: &str,
) -> Result<ReviewerDispatch, String> {
    journal::dispatch(receiver_root, task_id)
}
