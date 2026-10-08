//! Re-read received refs; callers run this off the app mutex.
use crate::reviews::model::*;
use crate::reviews::publication::{self, ReceivedCommit};
use crate::reviews::records::Review;
use crate::store::Store;
use crate::tracker::Actor;

#[path = "observations.rs"]
mod observations;
#[path = "recovery.rs"]
mod recovery;

#[derive(Debug)]
pub struct SyncResult {
    pub project_path: String,
    pub task_id: String,
    pub workspace_id: String,
    pub persisted: bool,
    pub retry: bool,
}

pub fn reconcile(store: &Store, task_id: &str) -> Result<SyncResult, String> {
    let task = store
        .load_tracker_task(task_id)
        .map_err(|e| e.to_string())?
        .ok_or("task missing")?;
    let mut review = store
        .load_review_sync_state(task_id)
        .map_err(|e| e.to_string())?
        .ok_or("review missing")?;
    let mut result = SyncResult {
        project_path: task.project_path,
        task_id: task_id.into(),
        workspace_id: review.workspace_id.clone(),
        persisted: false,
        retry: false,
    };
    if review.bindings.is_empty() {
        if store
            .load_review_sync_candidate(task_id)
            .map_err(|error| error.to_string())?
            .is_some()
        {
            return Err("pending review sync capture has no registered Git receiver".into());
        }
        return Ok(result);
    }
    let (mut journal, current, recovered) = match prepare(store, &review) {
        Ok(prepared) => prepared,
        Err(error) => return preparation_failure(store, &review, result, error),
    };
    review = current;
    result.persisted |= recovered;
    if !active(&review) {
        return Ok(result);
    }
    let received = observe_all(&review);
    let capture_error = update_snapshot(store, &mut review, &mut journal, &received, &mut result)?;
    persist_observations(
        store,
        &review,
        &received,
        capture_error.as_deref(),
        &mut result,
    )?;
    Ok(result)
}

fn active(review: &Review) -> bool {
    review
        .pull_request
        .as_ref()
        .is_some_and(|pr| pr.status.is_active())
}

fn observe_all(review: &Review) -> Vec<Result<ReceivedCommit, String>> {
    review
        .bindings
        .iter()
        .map(publication::observe_received)
        .collect()
}

fn prepare(
    store: &Store,
    review: &Review,
) -> Result<(recovery::CaptureJournal, Review, bool), String> {
    let journal = recovery::CaptureJournal::acquire(&review.task_id, &review.bindings)?;
    let recovered = journal.recover(store, &review.bindings)?;
    let current = store
        .load_review_sync_state(&review.task_id)
        .map_err(|error| error.to_string())?
        .ok_or("review missing")?;
    Ok((journal, current, recovered))
}

fn preparation_failure(
    store: &Store,
    review: &Review,
    mut result: SyncResult,
    error: String,
) -> Result<SyncResult, String> {
    if error.starts_with("busy:") || !active(review) {
        return Err(error);
    }
    result.retry = true;
    let received = observe_all(review);
    persist_observations(store, review, &received, Some(&error), &mut result)?;
    Ok(result)
}

fn update_snapshot(
    store: &Store,
    review: &mut Review,
    journal: &mut recovery::CaptureJournal,
    received: &[Result<ReceivedCommit, String>],
    result: &mut SyncResult,
) -> Result<Option<String>, String> {
    let tips: Result<Vec<_>, _> = received.iter().cloned().collect();
    let Ok(tips) = tips else {
        result.retry = true;
        return Ok(None);
    };
    if !vector_changed(review, &tips)? {
        return Ok(None);
    }
    match publish_locked(store, review, &tips, journal) {
        Ok((saved, cleanup_error)) => {
            *review = saved;
            result.persisted = true;
            result.retry |= cleanup_error.is_some();
            Ok(cleanup_error)
        }
        Err(error) => {
            result.retry = true;
            Ok(Some(error))
        }
    }
}

fn persist_observations(
    store: &Store,
    review: &Review,
    received: &[Result<ReceivedCommit, String>],
    capture_error: Option<&str>,
    result: &mut SyncResult,
) -> Result<(), String> {
    if let Err(error) = observations::persist(store, review, received, capture_error, result) {
        if !result.persisted {
            return Err(error);
        }
        eprintln!(
            "review sync: observation retry for {}: {error}",
            review.task_id
        );
        result.retry = true;
    }
    Ok(())
}

fn latest(review: &Review) -> Result<&ReviewSnapshot, String> {
    let id = review
        .pull_request
        .as_ref()
        .and_then(|pr| pr.latest_published_snapshot_id.as_ref())
        .ok_or("PR has no published snapshot")?;
    review
        .snapshots
        .iter()
        .find(|snapshot| &snapshot.id == id)
        .ok_or_else(|| "PR published snapshot is missing".into())
}

fn vector_changed(review: &Review, received: &[ReceivedCommit]) -> Result<bool, String> {
    let previous = latest(review)?;
    for (binding, tip) in review.bindings.iter().zip(received) {
        let directory = previous
            .directories
            .iter()
            .find(|dir| dir.id == binding.directory_id)
            .ok_or("PR membership is missing")?;
        if directory.head.as_deref() != Some(&tip.head)
            || directory.base.as_ref().map(|base| base.oid.as_str()) != Some(&tip.comparison_base)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
fn publish(store: &Store, review: &Review, received: &[ReceivedCommit]) -> Result<Review, String> {
    let mut journal = recovery::CaptureJournal::acquire(&review.task_id, &review.bindings)?;
    journal.recover(store, &review.bindings)?;
    publish_locked(store, review, received, &mut journal).map(|(saved, _)| saved)
}

fn publish_locked(
    store: &Store,
    review: &Review,
    received: &[ReceivedCommit],
    journal: &mut recovery::CaptureJournal,
) -> Result<(Review, Option<String>), String> {
    let snapshot_id = uuid::Uuid::new_v4().to_string();
    journal.begin(store, &snapshot_id)?;
    let captured = capture(review, &snapshot_id, received);
    if captured.is_ok() {
        recovery::checkpoint(&review.task_id, "before-db");
    }
    let result = captured.and_then(|snapshot| {
        let mut bindings = review.bindings.clone();
        for (binding, tip) in bindings.iter_mut().zip(received) {
            binding.last_received_head = Some(tip.head.clone());
            binding.publication = ReviewPublicationState::Published;
            binding.recovery = None;
        }
        publication::with_received_snapshot_locked(&bindings, &review.task_id, &snapshot_id, || {
            store
                .save_review_received_snapshot(&review.task_id, review.version, snapshot, &bindings)
                .map_err(|error| error.to_string())
        })
    });
    if result.is_ok() {
        recovery::checkpoint(&review.task_id, "after-db");
    }
    let cleanup_error = journal.finish(store, &review.bindings).err();
    if let Some(error) = &cleanup_error {
        eprintln!("review sync: retain refused candidate {snapshot_id}: {error}");
    }
    result.map(|saved| (saved, cleanup_error))
}

fn capture(
    review: &Review,
    snapshot_id: &str,
    received: &[ReceivedCommit],
) -> Result<ReviewSnapshot, String> {
    let previous = latest(review)?;
    let mut snapshot = ReviewSnapshot {
        id: snapshot_id.into(),
        number: 0,
        created_at: time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|e| e.to_string())?,
        author: Actor::Build,
        directories: previous.directories.clone(),
        publication: None,
    };
    let mut head_changed = false;
    let mut publications = Vec::with_capacity(received.len());
    for (binding, tip) in review.bindings.iter().zip(received) {
        let directory = snapshot
            .directories
            .iter_mut()
            .find(|dir| dir.id == binding.directory_id)
            .ok_or("PR membership is missing")?;
        head_changed |= directory.head.as_deref() != Some(&tip.head);
        publications.push(ReviewPublishedDirectory {
            directory_id: binding.directory_id.clone(),
            target_head: tip.target_head.clone(),
            rewritten: rewritten(binding, directory.head.as_deref(), &tip.head)?,
        });
        publication::capture_received_directory(
            &review.task_id,
            snapshot_id,
            binding,
            tip,
            directory,
        )?;
    }
    snapshot.publication = Some(ReviewSnapshotPublication {
        reason: if head_changed {
            ReviewPublicationReason::Received
        } else {
            ReviewPublicationReason::BaseChanged
        },
        directories: publications,
    });
    Ok(snapshot)
}

fn rewritten(
    binding: &ReviewBranchBinding,
    previous: Option<&str>,
    head: &str,
) -> Result<bool, String> {
    let Some(previous) = previous.filter(|previous| *previous != head) else {
        return Ok(false);
    };
    let repository =
        git2::Repository::open_bare(&binding.receiving_repository).map_err(|e| e.to_string())?;
    let head = git2::Oid::from_str(head).map_err(|e| e.to_string())?;
    let previous = git2::Oid::from_str(previous).map_err(|e| e.to_string())?;
    repository
        .graph_descendant_of(head, previous)
        .map(|descendant| !descendant)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
#[path = "tests.rs"]
pub(crate) mod tests;

#[cfg(test)]
#[path = "recovery_tests.rs"]
mod recovery_tests;
