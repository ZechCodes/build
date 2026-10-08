//! Review detail includes independently revised sync facts and durable merge journals.

use crate::reviews::records::{Review, ReviewMode};
use crate::store::Store;
use serde_json::{json, Value};

pub(super) fn result(store: &Store, review: Review) -> Result<Value, String> {
    let mut result = json!({ "review": review });
    if review.mode == ReviewMode::PullRequest {
        let sync = store
            .load_review_sync_observations(&review.task_id)
            .map_err(|error| error.to_string())?;
        let intents = store
            .load_review_merge_intents(&review.task_id)
            .map_err(|error| error.to_string())?;
        if !sync.is_empty() {
            result["sync"] = json!(sync);
        }
        if !intents.is_empty() {
            result["merge_intents"] = json!(intents);
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reviews::model::{ReviewMergeRequest, ReviewMergeSource};
    use crate::reviews::sync::reconcile::tests::Fixture;
    use crate::tracker::Actor;

    #[test]
    fn pr_projection_exposes_independent_sync_revisions_and_retained_merge_intents() {
        let fixture = Fixture::new();
        fixture.sync();
        let review = fixture.review();
        let before = result(&fixture.store, review.clone()).unwrap();
        assert_eq!(before["sync"][0]["revision"], 1);
        fixture.commit("pending-change.txt");
        fixture.sync();
        assert_eq!(fixture.review(), review, "pending work changes sync alone");
        let binding = &review.bindings[0];
        let snapshot = review.snapshots.last().unwrap();
        let mut merge = fixture
            .store
            .reserve_review_merge(
                &fixture.request.project_path,
                "projection-merge",
                ReviewMergeRequest {
                    task_id: review.task_id.clone(),
                    expected_version: review.version,
                    snapshot_id: snapshot.id.clone(),
                    actor: Actor::User,
                    sources: vec![ReviewMergeSource {
                        directory_id: binding.directory_id.clone(),
                        repository_id: binding.repository_id.clone(),
                        base_branch_ref: binding.base_branch_ref.clone(),
                        head: binding.last_received_head.clone().unwrap(),
                        expected_base_head: snapshot.directories[0]
                            .base
                            .as_ref()
                            .unwrap()
                            .oid
                            .clone(),
                        push: None,
                    }],
                },
            )
            .unwrap();
        merge.state = crate::reviews::model::ReviewMergeState::Interrupted;
        let merge = fixture
            .store
            .save_review_merge_intent(&merge, merge.version)
            .unwrap();
        let projected = result(&fixture.store, review.clone()).unwrap();
        assert_eq!(projected["review"], serde_json::to_value(review).unwrap());
        assert_eq!(projected["sync"][0]["revision"], 2);
        assert_eq!(projected["sync"][0]["health"], "pending");
        assert_eq!(projected["merge_intents"], json!([merge]));
    }

    #[test]
    fn legacy_projection_retains_exact_result_without_pr_observations() {
        let fixture = Fixture::new();
        fixture.sync();
        let mut review = fixture.review();
        review.mode = ReviewMode::Snapshot;
        let expected = json!({ "review": review });
        assert_eq!(result(&fixture.store, review).unwrap(), expected);
    }
}
