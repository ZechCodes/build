//! Independent observation revisions; timestamps alone never cause a write.
use super::since_review as walk;
use super::{latest, SyncResult};
use crate::reviews::model::*;
use crate::reviews::publication::{self, ReceivedCommit};
use crate::reviews::records::Review;
use crate::store::Store;

pub(super) fn persist(
    store: &Store,
    review: &Review,
    received: &[Result<ReceivedCommit, String>],
    capture_error: Option<&str>,
    result: &mut SyncResult,
) -> Result<(), String> {
    let prior = store
        .load_review_sync_observations(&review.task_id)
        .map_err(|e| e.to_string())?;
    let snapshot = latest(review)?;
    let reviewed = store
        .load_user_reviewed_snapshot(&review.task_id)
        .map_err(|e| e.to_string())?;
    for (binding, tip) in review.bindings.iter().zip(received) {
        let saved = snapshot
            .directories
            .iter()
            .find(|dir| dir.id == binding.directory_id)
            .ok_or("PR membership is missing")?;
        let mut observation = observe(
            &review.task_id,
            binding,
            saved.head.clone(),
            tip,
            capture_error,
        );
        let previous = prior
            .iter()
            .find(|obs| obs.directory_id == binding.directory_id);
        if let Some(reviewed) = &reviewed {
            since_review(&mut observation, binding, reviewed, previous);
        }
        result.retry |= matches!(
            observation.health,
            ReviewSyncHealth::Unavailable | ReviewSyncHealth::Interrupted
        );
        if unchanged(previous, &observation) {
            continue;
        }
        observation.observed_at = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|e| e.to_string())?;
        store
            .save_review_sync_observation(&observation, previous.map_or(0, |obs| obs.revision))
            .map_err(|e| e.to_string())?;
        result.persisted = true;
    }
    Ok(())
}

fn observe(
    task_id: &str,
    binding: &ReviewBranchBinding,
    snapshot_head: Option<String>,
    tip: &Result<ReceivedCommit, String>,
    capture_error: Option<&str>,
) -> ReviewSyncObservation {
    let working = git2::Repository::open(&binding.working_repository).ok();
    let working_head = working
        .as_ref()
        .and_then(|repo| repo.refname_to_id(&binding.dedicated_branch_ref).ok())
        .map(|oid| oid.to_string());
    let received_head = match tip {
        Ok(tip) => Some(tip.head.clone()),
        Err(_) => publication::registered_received_head(binding)
            .ok()
            .flatten(),
    };
    let pending_commits = pending(
        working.as_ref(),
        working_head.as_deref(),
        received_head.as_deref(),
    );
    let error = tip.as_ref().err().cloned().or_else(|| {
        working_head
            .is_none()
            .then(|| "review working branch is unavailable".into())
    });
    let (health, error) = health(
        error,
        capture_error,
        &received_head,
        &snapshot_head,
        &working_head,
    );
    ReviewSyncObservation {
        task_id: task_id.into(),
        directory_id: binding.directory_id.clone(),
        revision: 0,
        health,
        target_head: tip.as_ref().ok().map(|tip| tip.target_head.clone()),
        comparison_base: tip.as_ref().ok().map(|tip| tip.comparison_base.clone()),
        working_head,
        received_head,
        snapshot_head,
        pending_commits,
        reviewed_snapshot_id: None,
        commits_since_review: None,
        rewritten_since_review: false,
        observed_at: String::new(),
        error,
    }
}

fn health(
    error: Option<String>,
    capture_error: Option<&str>,
    received: &Option<String>,
    snapshot: &Option<String>,
    working: &Option<String>,
) -> (ReviewSyncHealth, Option<String>) {
    if error.is_some() {
        return (ReviewSyncHealth::Unavailable, error);
    }
    if let Some(error) = capture_error {
        return (ReviewSyncHealth::Interrupted, Some(error.into()));
    }
    if received != snapshot || working != received {
        (ReviewSyncHealth::Pending, None)
    } else {
        (ReviewSyncHealth::Current, None)
    }
}

fn pending(
    repository: Option<&git2::Repository>,
    working: Option<&str>,
    received: Option<&str>,
) -> Option<u64> {
    let repository = repository?;
    let working = git2::Oid::from_str(working?).ok()?;
    let received = git2::Oid::from_str(received?).ok()?;
    if working == received {
        return Some(0);
    }
    repository
        .graph_ahead_behind(working, received)
        .ok()
        .map(|(ahead, _)| ahead as u64)
}

/// Counted in the receiving repository, where snapshot pins keep a rewritten
/// reviewed head readable. A stored result for the same head and baseline is
/// reused, so idle polls walk no history (#453).
fn since_review(
    observation: &mut ReviewSyncObservation,
    binding: &ReviewBranchBinding,
    reviewed: &ReviewSnapshot,
    previous: Option<&ReviewSyncObservation>,
) {
    observation.reviewed_snapshot_id = Some(reviewed.id.clone());
    if let Some(previous) = previous.filter(|previous| reusable(previous, observation)) {
        observation.commits_since_review = previous.commits_since_review;
        observation.rewritten_since_review = previous.rewritten_since_review;
        return;
    }
    let reviewed_head = reviewed
        .directories
        .iter()
        .find(|directory| directory.id == binding.directory_id)
        .and_then(|directory| directory.head.as_deref())
        .and_then(|head| git2::Oid::from_str(head).ok());
    let head = observation
        .received_head
        .as_deref()
        .and_then(|head| git2::Oid::from_str(head).ok());
    let (Some(reviewed_head), Some(head)) = (reviewed_head, head) else {
        return;
    };
    let Ok(repository) = git2::Repository::open_bare(&binding.receiving_repository) else {
        return;
    };
    match walk::since_review(&repository, head, reviewed_head, walk::WALK_LIMIT) {
        Some(walk::SinceReview::Count(count)) => observation.commits_since_review = Some(count),
        Some(walk::SinceReview::Rewritten) => observation.rewritten_since_review = true,
        None => {}
    }
}

/// An absent result (budget spent, unreadable) is retried on the next pass.
fn reusable(previous: &ReviewSyncObservation, observation: &ReviewSyncObservation) -> bool {
    previous.reviewed_snapshot_id == observation.reviewed_snapshot_id
        && previous.received_head == observation.received_head
        && (previous.commits_since_review.is_some() || previous.rewritten_since_review)
}

fn unchanged(
    previous: Option<&ReviewSyncObservation>,
    observation: &ReviewSyncObservation,
) -> bool {
    let Some(previous) = previous else {
        return false;
    };
    let mut previous = previous.clone();
    previous.revision = 0;
    previous.observed_at.clear();
    &previous == observation
}
