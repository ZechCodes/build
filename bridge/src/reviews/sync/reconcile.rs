//! Re-read received refs; callers run this off the app mutex.
use crate::reviews::model::*;
use crate::reviews::publication::{self, ReceivedCommit};
use crate::reviews::records::Review;
use crate::store::Store;
use crate::tracker::Actor;

#[path = "observations.rs"]
mod observations;

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
    if !review
        .pull_request
        .as_ref()
        .is_some_and(|pr| pr.status.is_active())
    {
        return Ok(result);
    }
    let received: Vec<_> = review
        .bindings
        .iter()
        .map(publication::observe_received)
        .collect();
    let observed: Result<Vec<_>, _> = received.iter().cloned().collect();
    let mut capture_error = None;
    if let Ok(tips) = observed {
        if vector_changed(&review, &tips)? {
            match publish(store, &review, &tips) {
                Ok(saved) => {
                    review = saved;
                    result.persisted = true;
                }
                Err(error) => {
                    capture_error = Some(error);
                    result.retry = true;
                }
            }
        }
    } else {
        result.retry = true;
    }
    if let Err(error) = observations::persist(
        store,
        &review,
        &received,
        capture_error.as_deref(),
        &mut result,
    ) {
        // A committed snapshot or observation must still be announced even
        // when another writer wins a subsequent observation revision.
        if !result.persisted {
            return Err(error);
        }
        eprintln!("review sync: observation retry for {task_id}: {error}");
        result.retry = true;
    }
    Ok(result)
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

fn publish(store: &Store, review: &Review, received: &[ReceivedCommit]) -> Result<Review, String> {
    let snapshot_id = uuid::Uuid::new_v4().to_string();
    let captured = capture(review, &snapshot_id, received);
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
    if result.is_err() {
        if let Err(error) =
            publication::cleanup_received_pins(&review.task_id, &snapshot_id, &review.bindings)
        {
            eprintln!("review sync: retain refused snapshot pins {snapshot_id}: {error}");
        }
    }
    result
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
