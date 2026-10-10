//! Explicit current-version/ref confirmation resumes one retained merge vector.
use super::*;

pub(super) fn plan(
    store: &Store,
    review: &Review,
    intent: &ReviewMergeIntent,
    request: &ReviewMergeRequest,
) -> Result<ReviewMergeIntent, String> {
    let mut confirmed = intent.clone();
    confirmed.request = request.clone();
    // Every submitted target is locked and checked, including sources already
    // integrated. Their saved successful action and original publication stay.
    fence::with_refs(review, &confirmed, true, || {
        verify_successes(review, intent)?;
        store
            .refresh_review_merge_plan(intent, request)
            .map_err(|error| error.to_string())
    })
}

fn verify_successes(review: &Review, intent: &ReviewMergeIntent) -> Result<(), String> {
    for source in &intent.request.sources {
        if successful_merge(review, intent, &source.directory_id).is_none() {
            continue;
        }
        let binding = review
            .bindings
            .iter()
            .find(|binding| binding.directory_id == source.directory_id)
            .ok_or("missing PR binding")?;
        super::super::git_actions::verify_integrated(
            &binding.source_repository,
            &source.base_branch_ref,
            &source.head,
        )?;
    }
    Ok(())
}
