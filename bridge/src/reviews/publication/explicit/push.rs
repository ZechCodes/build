use super::*;
use crate::reviews::publication::{self, PublicationError};
use crate::reviews::receivers::{self, refs};
use crate::reviews::sync::reconcile::{reconcile_held_as, recovery::CaptureJournal};
use std::cell::Cell;

const RECOVERY: &str = "Refresh review and receiver state before retrying publication.";

struct PublicationSuccess {
    changed: bool,
    recovery: Option<String>,
}

pub(super) fn push(
    store: &Store,
    request: &PushRequest,
    check: &dyn Fn() -> Result<(), String>,
) -> Result<PushResult, String> {
    let review = active(store, &request.task_id, request.expected_version)?;
    selections(
        &review,
        request
            .sources
            .iter()
            .map(|source| source.directory_id.as_str()),
    )?;
    check()?;
    let mut journal = CaptureJournal::acquire(&review.task_id, &review.bindings)?;
    journal.recover(store, &review.bindings)?;
    let review = active(store, &request.task_id, request.expected_version)?;
    let mut sources = Vec::with_capacity(request.sources.len());
    for source in &request.sources {
        let binding = review
            .bindings
            .iter()
            .find(|binding| binding.directory_id == source.directory_id)
            .ok_or("invalid review params: Git directory is not bound")?;
        let result = publish_one(store, request, source, binding, check);
        sources.push(outcome(source, result));
    }
    let recovery = match reconcile_held_as(store, &request.task_id, &mut journal, &request.actor) {
        Ok(result) if result.retry => Some(
            "Publication received; snapshot reconciliation needs recovery. Refresh the review."
                .into(),
        ),
        Ok(_) => None,
        Err(error) => Some(format!(
            "Publication received; reconciliation needs recovery: {}",
            crate::source_sync::without_credentials(&error)
        )),
    };
    Ok(PushResult {
        review: load_full(store, &request.task_id)?,
        sources,
        recovery,
    })
}

fn publish_one(
    store: &Store,
    request: &PushRequest,
    source: &PushSource,
    binding: &ReviewBranchBinding,
    check: &dyn Fn() -> Result<(), String>,
) -> Result<PublicationSuccess, PublicationError> {
    receivers::validate_binding_receiver(binding).map_err(PublicationError::Failed)?;
    publication::validate_bound_remote(binding).map_err(PublicationError::Failed)?;
    let head = oid(&source.expected_head).map_err(PublicationError::Failed)?;
    let expected = source
        .expected_received_head
        .as_deref()
        .map(oid)
        .transpose()
        .map_err(PublicationError::Failed)?;
    let working = git2::Repository::open(&binding.working_repository)
        .map_err(|error| PublicationError::Failed(error.to_string()))?;
    validate_head(&working, binding, head).map_err(PublicationError::Failed)?;
    publication::import_publication_commit(
        &binding.receiving_repository,
        &binding.working_repository,
        &head.to_string(),
    )?;
    check().map_err(PublicationError::Failed)?;
    let receiver = git2::Repository::open_bare(&binding.receiving_repository)
        .map_err(|error| PublicationError::Failed(error.to_string()))?;
    validate_fast_forward(&receiver, expected, head, source.force_with_lease)
        .map_err(PublicationError::Failed)?;
    publish_locked(
        store,
        request,
        binding,
        &working,
        &receiver,
        (expected, head),
        check,
    )
}

fn publish_locked(
    store: &Store,
    request: &PushRequest,
    binding: &ReviewBranchBinding,
    working: &git2::Repository,
    receiver: &git2::Repository,
    heads: (Option<git2::Oid>, git2::Oid),
    check: &dyn Fn() -> Result<(), String>,
) -> Result<PublicationSuccess, PublicationError> {
    let (expected, head) = heads;
    let applied = Cell::new(false);
    let result = refs::with_expected_reference_locked(
        working,
        &binding.dedicated_branch_ref,
        head,
        |packed| {
            validate_head(working, binding, head)?;
            let mut recovery = None;
            let changed = refs::update_expected_reference_checked(
                receiver,
                &binding.receiving_ref,
                expected,
                head,
                |publish| {
                    check()?;
                    validate_head(working, binding, head)?;
                    receivers::validate_binding_receiver(binding)?;
                    publication::validate_bound_remote(binding)?;
                    let expectation = prepare_tracking(
                        store, request, binding, working, receiver, packed, heads,
                    )?;
                    check()?;
                    validate_head(working, binding, head)?;
                    let tracking_error = store
                        .with_review_publication_version(
                            &request.task_id,
                            request.expected_version,
                            || {
                                validate_head(working, binding, head)?;
                                publish()?;
                                applied.set(expected != Some(head));
                                Ok(publication::tracking::advance(
                                    working,
                                    binding,
                                    packed,
                                    &expectation.claim_fingerprint,
                                    expectation.expected_tracking_head.as_deref(),
                                    &expectation.target_head,
                                )
                                .err())
                            },
                        )
                        .map_err(|error| error.to_string())?;
                    let tracking_error = match tracking_error {
                        Some(error) => Some(error),
                        None => store
                            .clear_review_tracking_expectation(
                                &request.task_id,
                                request.expected_version,
                                &binding.directory_id,
                                &expectation.token,
                            )
                            .err()
                            .map(|error| error.to_string()),
                    };
                    recovery = tracking_error.map(tracking_recovery);
                    Ok(())
                },
            )?;
            Ok(PublicationSuccess { changed, recovery })
        },
    );
    result.map_err(|error| {
        if applied.get() {
            PublicationError::Interrupted(error)
        } else {
            PublicationError::Failed(error)
        }
    })
}

fn prepare_tracking(
    store: &Store,
    request: &PushRequest,
    binding: &ReviewBranchBinding,
    working: &git2::Repository,
    receiver: &git2::Repository,
    packed: &refs::PackedReferenceLease<'_>,
    heads: (Option<git2::Oid>, git2::Oid),
) -> Result<crate::store::TrackingExpectation, String> {
    let (expected, head) = heads;
    let fingerprint = publication::tracking::fingerprint(working, binding)?;
    let previous = store
        .load_review_tracking_expectation(
            &request.task_id,
            request.expected_version,
            &binding.directory_id,
        )
        .map_err(|error| error.to_string())?;
    if let Some(previous) = previous {
        if previous.target_head == head.to_string() {
            let matching = store
                .with_review_publication_version(&request.task_id, request.expected_version, || {
                    validate_head(working, binding, head)?;
                    matching_receiver_tracking(
                        working,
                        binding,
                        packed,
                        expected,
                        &previous.claim_fingerprint,
                    )
                })
                .map_err(|error| error.to_string());
            match matching {
                Ok(Some(matching)) => {
                    return replace_tracking(
                        store,
                        request,
                        binding,
                        &previous,
                        Some(&matching),
                        heads,
                    )
                }
                Ok(None) => {}
                Err(_) if expected == Some(head) => {}
                Err(error) => return Err(error),
            }
        }
        if previous.target_head != head.to_string() {
            let expected_tracking = store
                .with_review_publication_version(&request.task_id, request.expected_version, || {
                    validate_head(working, binding, head)?;
                    recover_tracking_expectation(
                        working, receiver, binding, packed, heads, &previous,
                    )
                })
                .map_err(|error| error.to_string())?;
            return replace_tracking(
                store,
                request,
                binding,
                &previous,
                expected_tracking.as_deref(),
                heads,
            );
        }
    }
    store
        .prepare_review_tracking_expectation(
            &request.task_id,
            request.expected_version,
            &binding.directory_id,
            &fingerprint,
            expected.as_ref().map(|head| head.to_string()).as_deref(),
            &head.to_string(),
        )
        .map_err(|error| error.to_string())
}

fn replace_tracking(
    store: &Store,
    request: &PushRequest,
    binding: &ReviewBranchBinding,
    previous: &crate::store::TrackingExpectation,
    expected_tracking: Option<&str>,
    heads: (Option<git2::Oid>, git2::Oid),
) -> Result<crate::store::TrackingExpectation, String> {
    let (expected, head) = heads;
    store
        .replace_review_tracking_expectation(
            &request.task_id,
            request.expected_version,
            &binding.directory_id,
            &previous.token,
            (
                expected_tracking,
                expected.as_ref().map(|head| head.to_string()).as_deref(),
            ),
            &head.to_string(),
        )
        .map_err(|error| error.to_string())
}

fn matching_receiver_tracking(
    working: &git2::Repository,
    binding: &ReviewBranchBinding,
    packed: &refs::PackedReferenceLease<'_>,
    received: Option<git2::Oid>,
    fingerprint: &str,
) -> Result<Option<String>, String> {
    let Some(received) = received else {
        return Ok(None);
    };
    let received = received.to_string();
    Ok(
        publication::tracking::matches_tip(working, binding, packed, fingerprint, &received)?
            .then_some(received),
    )
}

fn recover_tracking_expectation(
    working: &git2::Repository,
    receiver: &git2::Repository,
    binding: &ReviewBranchBinding,
    packed: &refs::PackedReferenceLease<'_>,
    heads: (Option<git2::Oid>, git2::Oid),
    previous: &crate::store::TrackingExpectation,
) -> Result<Option<String>, String> {
    let (received, _) = heads;
    if let Some(matching) = matching_receiver_tracking(
        working,
        binding,
        packed,
        received,
        &previous.claim_fingerprint,
    )? {
        return Ok(Some(matching));
    }
    if target_was_received(receiver, received, &previous.target_head)? {
        publication::tracking::advance(
            working,
            binding,
            packed,
            &previous.claim_fingerprint,
            previous.expected_tracking_head.as_deref(),
            &previous.target_head,
        )?;
        return Ok(Some(previous.target_head.clone()));
    }
    publication::tracking::validate_tip(
        working,
        binding,
        packed,
        &previous.claim_fingerprint,
        previous.expected_tracking_head.as_deref(),
    )?;
    Ok(previous.expected_tracking_head.clone())
}

fn target_was_received(
    receiver: &git2::Repository,
    received: Option<git2::Oid>,
    target: &str,
) -> Result<bool, String> {
    let Some(received) = received else {
        return Ok(false);
    };
    let target = oid(target)?;
    if received == target {
        return Ok(true);
    }
    match receiver.graph_descendant_of(received, target) {
        Ok(found) => Ok(found),
        Err(error) if error.code() == git2::ErrorCode::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

fn oid(value: &str) -> Result<git2::Oid, String> {
    if value.len() != 40 {
        return Err("invalid review params: expected head must be a full commit OID".into());
    }
    git2::Oid::from_str(value).map_err(|error| error.to_string())
}

fn validate_head(
    repository: &git2::Repository,
    binding: &ReviewBranchBinding,
    expected: git2::Oid,
) -> Result<(), String> {
    let head = repository.head().map_err(|error| error.to_string())?;
    if head.name() != Some(&binding.dedicated_branch_ref)
        || head.target() != Some(expected)
        || repository.refname_to_id(&binding.dedicated_branch_ref).ok() != Some(expected)
    {
        return Err("stale: review working branch or expected head changed".into());
    }
    repository
        .find_commit(expected)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn validate_fast_forward(
    repository: &git2::Repository,
    expected: Option<git2::Oid>,
    head: git2::Oid,
    force: bool,
) -> Result<(), String> {
    if force || expected.is_none() || expected == Some(head) {
        return Ok(());
    }
    if !repository
        .graph_descendant_of(head, expected.expect("received head checked"))
        .map_err(|error| error.to_string())?
    {
        return Err("conflict: review push would rewrite received history; use force_with_lease with the exact received head".into());
    }
    Ok(())
}

fn outcome(
    source: &PushSource,
    result: Result<PublicationSuccess, PublicationError>,
) -> PushOutcome {
    match result {
        Ok(success) => PushOutcome {
            directory_id: source.directory_id.clone(),
            status: if success.changed {
                PushStatus::Published
            } else {
                PushStatus::Unchanged
            },
            head: Some(source.expected_head.to_ascii_lowercase()),
            error: None,
            recovery: success.recovery,
        },
        Err(error) => PushOutcome {
            directory_id: source.directory_id.clone(),
            status: if matches!(error, PublicationError::Interrupted(_)) {
                PushStatus::Interrupted
            } else {
                PushStatus::Failed
            },
            head: None,
            error: Some(crate::source_sync::without_credentials(&error.to_string())),
            recovery: Some(RECOVERY.into()),
        },
    }
}

fn tracking_recovery(error: String) -> String {
    format!(
        "Received publication succeeded; owned upstream tracking needs recovery: {} {RECOVERY}",
        crate::source_sync::without_credentials(&error)
    )
}
