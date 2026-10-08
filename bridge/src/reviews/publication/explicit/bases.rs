use super::*;
use crate::reviews::model::{ReviewPublicationReason, ReviewPublicationState};
use crate::reviews::publication;
use crate::reviews::receivers::{self, locks::acquire_git_lock};
use crate::reviews::sync::reconcile::{capture, recovery::CaptureJournal};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub(super) fn update(
    store: &Store,
    request: &BaseRequest,
    check: &dyn Fn() -> Result<(), String>,
) -> Result<Review, String> {
    let review = active(store, &request.task_id, request.expected_version)?;
    selections(
        &review,
        request.bases.iter().map(|base| base.directory_id.as_str()),
    )?;
    check()?;
    let mut journal = CaptureJournal::acquire(&review.task_id, &review.bindings)?;
    journal.recover(store, &review.bindings)?;
    let mut review = active(store, &request.task_id, request.expected_version)?;
    for base in &request.bases {
        let binding = review
            .bindings
            .iter_mut()
            .find(|binding| binding.directory_id == base.directory_id)
            .ok_or("invalid review params: retarget directory is not bound")?;
        binding.base_branch_ref = local_branch(&base.branch)?;
        publication::observe_received(binding)?;
    }
    let snapshot_id = uuid::Uuid::new_v4().to_string();
    journal.begin(store, &snapshot_id)?;
    let saved = publish(store, request, &review, &snapshot_id, check);
    if let Err(error) = journal.finish(store, &review.bindings) {
        eprintln!("review retarget: retain candidate {snapshot_id} for recovery: {error}");
    }
    saved?;
    load_full(store, &request.task_id)
}

fn local_branch(branch: &str) -> Result<String, String> {
    let name = if branch.starts_with("refs/") {
        branch.into()
    } else {
        format!("refs/heads/{branch}")
    };
    if branch == "HEAD"
        || !name.starts_with("refs/heads/")
        || !git2::Reference::is_valid_name(&name)
    {
        return Err("invalid review params: retarget needs a literal local base branch".into());
    }
    Ok(name)
}

fn publish(
    store: &Store,
    request: &BaseRequest,
    review: &Review,
    snapshot_id: &str,
    check: &dyn Fn() -> Result<(), String>,
) -> Result<Review, String> {
    let tips = review
        .bindings
        .iter()
        .map(publication::observe_received)
        .collect::<Result<Vec<_>, _>>()?;
    let mut snapshot = capture(review, snapshot_id, &tips)?;
    snapshot.author = request.actor.clone();
    snapshot
        .publication
        .as_mut()
        .ok_or("retarget snapshot has no publication context")?
        .reason = ReviewPublicationReason::BaseChanged;
    let mut bindings = review.bindings.clone();
    for (binding, tip) in bindings.iter_mut().zip(&tips) {
        binding.last_received_head = Some(tip.head.clone());
        binding.publication = ReviewPublicationState::Published;
        binding.recovery = None;
    }
    publication::with_received_snapshot_locked(&bindings, &review.task_id, snapshot_id, || {
        check()?;
        with_targets_locked(&bindings, &tips, || {
            store
                .save_review_retargeted_snapshot(
                    &request.task_id,
                    request.expected_version,
                    snapshot,
                    &bindings,
                )
                .map_err(|error| error.to_string())
        })
    })
}

fn with_targets_locked<T>(
    bindings: &[ReviewBranchBinding],
    tips: &[publication::ReceivedCommit],
    publish: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let mut grouped: BTreeMap<PathBuf, BTreeMap<String, String>> = BTreeMap::new();
    for (binding, expected) in bindings.iter().zip(tips) {
        let repository = receivers::canonical_common_git_dir(&binding.source_repository)?;
        let previous = grouped.entry(repository).or_default().insert(
            binding.base_branch_ref.clone(),
            expected.target_head.clone(),
        );
        if previous.is_some_and(|previous| previous != expected.target_head) {
            return Err("stale: shared source base changed between retarget observations; refresh and retry".into());
        }
    }
    let mut guards = Vec::new();
    for (path, references) in grouped {
        let (guard, _) = acquire_git_lock(&path, Path::new("packed-refs.lock"))?;
        guards.push(guard);
        let repository = git2::Repository::open(&path).map_err(|error| error.to_string())?;
        for (reference, expected) in references {
            let (guard, _) = acquire_git_lock(&path, Path::new(&format!("{reference}.lock")))?;
            guards.push(guard);
            if repository
                .refname_to_id(&reference)
                .map_err(|error| error.to_string())?
                .to_string()
                != expected
            {
                return Err(
                    "stale: selected base branch changed during retarget; refresh and retry".into(),
                );
            }
        }
    }
    for guard in &guards {
        guard.verify_owned()?;
    }
    let result = publish();
    drop(guards);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_fixture::git_in;
    use crate::reviews::sync::reconcile::tests::Fixture;

    #[test]
    fn shared_source_target_movement_between_observations_rejects_the_whole_vector() {
        let fixture = Fixture::new();
        let first = fixture.review().bindings[0].clone();
        let old = publication::observe_received(&first).unwrap();
        let mut second = first.clone();
        second.directory_id = "directory-2".into();
        let target = fixture.commit("new-target.txt");
        git_in(
            &fixture.source,
            &["update-ref", &first.base_branch_ref, &target],
        );
        let new = publication::observe_received(&second).unwrap();
        assert_ne!(old.target_head, new.target_head);
        let published = std::cell::Cell::new(false);
        let result = with_targets_locked(&[first, second], &[old, new], || {
            published.set(true);
            Ok(())
        });
        assert!(result.unwrap_err().starts_with("stale:"));
        assert!(!published.get());
    }
}
