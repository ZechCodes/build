//! Durable task review metadata. Git content stays in each source repository.

use super::actions::{ReviewAction, ReviewDestination};
use super::model::ReviewSnapshot;
pub use super::model::{
    PullRequestMetadata, PullRequestStatus, ReviewBranchBinding, ReviewMembership,
    ReviewMembershipKind, ReviewMergeIntent, ReviewMergePush, ReviewMergeRequest,
    ReviewMergeSource, ReviewMergeState, ReviewMode, ReviewOpening, ReviewOpeningRequest,
    ReviewOpeningState, ReviewPreparationState, ReviewPublicationState, ReviewSummary,
    ReviewSyncHealth, ReviewSyncObservation,
};
use crate::tracker::Actor;
use serde::{Deserialize, Serialize};

/// The maximum size of a review's action description after trimming.
pub const MAX_REVIEW_DESCRIPTION_BYTES: usize = 2_000;

/// Keep MCP and storage at the same UTF-8 byte boundary.
pub fn review_description(description: &str) -> Option<&str> {
    let description = description.trim();
    (!description.is_empty() && description.len() <= MAX_REVIEW_DESCRIPTION_BYTES)
        .then_some(description)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewState {
    Open,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewCompletion {
    pub actor: Actor,
    pub description: String,
    pub completed_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Review {
    pub task_id: String,
    pub workspace_id: String,
    pub version: u64,
    pub state: ReviewState,
    pub snapshots: Vec<ReviewSnapshot>,
    pub completion: Option<ReviewCompletion>,
    #[serde(default)]
    pub actions: Vec<ReviewAction>,
    #[serde(default)]
    pub destinations: Vec<ReviewDestination>,
    #[serde(default, skip_serializing_if = "ReviewMode::is_snapshot")]
    pub mode: ReviewMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_request: Option<PullRequestMetadata>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bindings: Vec<ReviewBranchBinding>,
}

impl Review {
    pub fn summary(&self) -> Option<ReviewSummary> {
        if self.mode != ReviewMode::PullRequest {
            return None;
        }
        self.pull_request.as_ref().map(|metadata| ReviewSummary {
            task_id: self.task_id.clone(),
            workspace_id: self.workspace_id.clone(),
            version: self.version,
            status: metadata.status,
            latest_published_snapshot_id: metadata.latest_published_snapshot_id.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn legacy_review() -> &'static str {
        r#"{"task_id":"task-1","workspace_id":"workspace-1","version":4,"state":"open","snapshots":[],"completion":null,"actions":[],"destinations":[]}"#
    }

    fn pull_request_review() -> Review {
        let mut value: Value = serde_json::from_str(legacy_review()).unwrap();
        value["mode"] = json!("pull_request");
        value["pull_request"] = json!({
            "status": "changes_requested",
            "creator": {"kind": "user"},
            "originating_workspace_id": "workspace-original",
            "latest_published_snapshot_id": "snapshot-1",
            "directories": [
                {"directory_id": "dir-git", "source_id": "source-git", "kind": "git"},
                {"directory_id": "dir-live", "source_id": "source-live", "kind": "live"},
                {"directory_id": "dir-out", "source_id": "source-out", "kind": "excluded", "reason": "No commits"}
            ]
        });
        value["bindings"] = json!([binding_value()]);
        serde_json::from_value(value).unwrap()
    }

    fn binding_value() -> Value {
        json!({
            "directory_id": "dir-git",
            "source_id": "source-git",
            "repository_id": "repository-1",
            "working_repository": "/work/project",
            "source_repository": "/source/project",
            "initial_head": "opening-head",
            "dedicated_branch_ref": "refs/heads/review/task-1",
            "base_branch_ref": "refs/heads/main",
            "receiving_repository": "/receive/project",
            "receiving_ref": "refs/build/reviews/task-1/received",
            "remote_name": "build-review-task-1",
            "preparation": "planned",
            "publication": "pending"
        })
    }

    #[test]
    fn legacy_review_defaults_preserve_serialized_shape() {
        let review: Review = serde_json::from_str(legacy_review()).unwrap();
        assert_eq!(review.mode, ReviewMode::Snapshot);
        assert!(review.pull_request.is_none());
        assert!(review.bindings.is_empty());
        assert_eq!(serde_json::to_string(&review).unwrap(), legacy_review());
        assert_eq!(review.summary(), None);

        let mut earliest: Value = serde_json::from_str(legacy_review()).unwrap();
        earliest.as_object_mut().unwrap().remove("actions");
        earliest.as_object_mut().unwrap().remove("destinations");
        assert_eq!(serde_json::from_value::<Review>(earliest).unwrap(), review);
    }

    #[test]
    fn pull_request_identity_and_bindings_roundtrip() {
        let review = pull_request_review();
        assert_eq!(review.mode, ReviewMode::PullRequest);
        let metadata = review.pull_request.as_ref().unwrap();
        assert_eq!(metadata.creator, Actor::User);
        assert_eq!(metadata.originating_workspace_id, "workspace-original");
        assert_eq!(metadata.directories[0].kind, ReviewMembershipKind::Git);
        assert_eq!(metadata.directories[1].kind, ReviewMembershipKind::Live);
        assert_eq!(metadata.directories[2].kind, ReviewMembershipKind::Excluded);
        assert_eq!(
            metadata.directories[2].reason.as_deref(),
            Some("No commits")
        );
        assert_eq!(
            review.bindings[0].preparation,
            ReviewPreparationState::Planned
        );
        assert_eq!(
            review.bindings[0].publication,
            ReviewPublicationState::Pending
        );
        assert_eq!(review.bindings[0].initial_head, "opening-head");
        assert_eq!(
            serde_json::from_value::<Review>(serde_json::to_value(&review).unwrap()).unwrap(),
            review
        );
        assert_eq!(
            review.summary(),
            Some(ReviewSummary {
                task_id: "task-1".into(),
                workspace_id: "workspace-1".into(),
                version: 4,
                status: PullRequestStatus::ChangesRequested,
                latest_published_snapshot_id: Some("snapshot-1".into()),
            })
        );
    }

    #[test]
    fn branch_bindings_require_the_committed_opening_head() {
        let mut binding = binding_value();
        binding.as_object_mut().unwrap().remove("initial_head");
        let error = serde_json::from_value::<ReviewBranchBinding>(binding).unwrap_err();
        assert!(error.to_string().contains("missing field `initial_head`"));
    }

    #[test]
    fn snapshot_mode_never_projects_a_pull_request_summary() {
        let mut review = pull_request_review();
        review.mode = ReviewMode::Snapshot;
        assert_eq!(review.summary(), None);
        review.mode = ReviewMode::PullRequest;
        review.pull_request = None;
        assert_eq!(review.summary(), None);
    }

    #[test]
    fn active_pull_request_statuses_are_explicit() {
        for (status, name, active) in [
            (PullRequestStatus::Open, "open", true),
            (
                PullRequestStatus::ChangesRequested,
                "changes_requested",
                true,
            ),
            (PullRequestStatus::Approved, "approved", true),
            (PullRequestStatus::Merged, "merged", false),
            (PullRequestStatus::Closed, "closed", false),
        ] {
            assert_eq!(status.is_active(), active);
            assert_eq!(serde_json::to_value(status).unwrap(), json!(name));
        }
    }

    #[test]
    fn incomplete_opening_retains_workspace_ownership_until_cancelled() {
        for (state, name, claims_workspace) in [
            (ReviewOpeningState::Preparing, "preparing", true),
            (ReviewOpeningState::Published, "published", false),
            (ReviewOpeningState::Interrupted, "interrupted", true),
            (ReviewOpeningState::Failed, "failed", true),
            (ReviewOpeningState::Cancelled, "cancelled", false),
        ] {
            assert_eq!(state.is_claiming_workspace(), claims_workspace);
            assert_eq!(serde_json::to_value(state).unwrap(), json!(name));
        }
    }

    #[test]
    fn opening_operation_roundtrips_the_exact_request_and_recovery_binding() {
        let mut binding = binding_value();
        binding["original_branch_ref"] = json!("refs/heads/feature");
        binding["last_received_head"] = json!("received-head");
        binding["preparation"] = json!("remote_configured");
        binding["publication"] = json!("interrupted");
        binding["recovery"] = json!("Check receiving ref before publishing");
        let value = json!({
            "project_path": "/source/project",
            "request_id": "open-request-1",
            "version": 2,
            "request": {
                "workspace_id": "workspace-1",
                "title": "Review changes",
                "description": "Review the committed work",
                "creator": {"kind": "agent", "agent_id": "agent-1"},
                "reviewer": {"kind": "project_agent"},
                "directories": [{"directory_id": "dir-git", "source_id": "source-git", "kind": "git"}],
                "base_branches": {"dir-git": "main"}
            },
            "task": {
                "id": "task-1", "project_path": "/source/project", "number": 1,
                "title": "Review changes", "body": "Review the committed work",
                "state": "open", "status": "in_review", "created_by": {"kind": "user"},
                "created_at": "created", "updated_at": "updated"
            },
            "state": "interrupted", "bindings": [binding],
            "created_at": "created", "updated_at": "updated", "error": "Publish interrupted"
        });
        let opening: ReviewOpening = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(opening.request.base_branches["dir-git"], "main");
        assert_eq!(
            opening.request.reviewer,
            Some(crate::tracker::Assignee::ProjectAgent)
        );
        let serialized = serde_json::to_value(&opening).unwrap();
        assert_eq!(serialized["request"], value["request"]);
        assert_eq!(serialized["bindings"], value["bindings"]);
        assert_eq!(serialized["bindings"][0]["initial_head"], "opening-head");
        assert_eq!(
            serde_json::from_value::<ReviewOpening>(serialized).unwrap(),
            opening
        );
    }

    #[test]
    fn merge_operation_roundtrips_concurrency_preconditions_and_action_ids() {
        let value = json!({
            "project_path": "/source/project", "request_id": "merge-request-1", "version": 3,
            "request": {
                "task_id": "task-1", "expected_version": 4, "snapshot_id": "snapshot-1",
                "actor": {"kind": "user"},
                "sources": [{
                    "directory_id": "dir-git", "repository_id": "repository-1",
                    "base_branch_ref": "refs/heads/main", "head": "review-head",
                    "expected_base_head": "base-head", "push": {"remote": "origin", "branch": "main"}
                }]
            },
            "state": "running", "action_ids": ["action-1", "action-2"],
            "created_at": "created", "updated_at": "updated"
        });
        let merge: ReviewMergeIntent = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(merge.request.expected_version, 4);
        assert_eq!(merge.request.sources[0].expected_base_head, "base-head");
        assert_eq!(serde_json::to_value(&merge).unwrap(), value);
        assert_eq!(
            serde_json::from_value::<ReviewMergeIntent>(value).unwrap(),
            merge
        );
        for (state, name) in [
            (ReviewMergeState::Running, "running"),
            (ReviewMergeState::Succeeded, "succeeded"),
            (ReviewMergeState::Failed, "failed"),
            (ReviewMergeState::Interrupted, "interrupted"),
        ] {
            assert_eq!(serde_json::to_value(state).unwrap(), json!(name));
        }
    }

    #[test]
    fn sync_observations_are_separate_from_review_identity() {
        let value = json!({
            "task_id": "task-1", "directory_id": "dir-git", "revision": 8,
            "health": "current", "working_head": "working", "received_head": "received",
            "snapshot_head": "published", "pending_commits": 2, "observed_at": "now"
        });
        let observation: ReviewSyncObservation = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(observation.revision, 8);
        assert_eq!(observation.pending_commits, Some(2));
        assert_eq!(serde_json::to_value(&observation).unwrap(), value);
        assert_eq!(
            serde_json::from_value::<ReviewSyncObservation>(value).unwrap(),
            observation
        );
        let review_value = serde_json::to_value(pull_request_review()).unwrap();
        assert!(review_value.get("sync").is_none());
        assert!(review_value.get("revision").is_none());
        for (health, name) in [
            (ReviewSyncHealth::Pending, "pending"),
            (ReviewSyncHealth::Current, "current"),
            (ReviewSyncHealth::Unavailable, "unavailable"),
            (ReviewSyncHealth::Interrupted, "interrupted"),
        ] {
            assert_eq!(serde_json::to_value(health).unwrap(), json!(name));
        }
    }
}
