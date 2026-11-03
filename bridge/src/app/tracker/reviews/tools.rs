use crate::api::v1::reviews::ReviewDiffParams;
use crate::app::AppState;
use crate::mcp::BridgeAction;
use crate::reviews::read::ReviewReadRequest;
use crate::tracker::Actor;
use serde_json::Value;

impl AppState {
    pub(in crate::app) fn review_surface_action(
        &mut self,
        entity_id: &str,
        agent_id: &str,
        action: &BridgeAction,
    ) -> Option<Result<Value, String>> {
        if let BridgeAction::TrackerOpenReview { params } = action {
            return Some(self.scoped_review_open(entity_id, agent_id, params));
        }
        let task_id = review_task_id(action)?;
        Some(self.scoped_review_action(entity_id, agent_id, task_id, action))
    }

    fn scoped_review_open(
        &mut self,
        entity_id: &str,
        agent_id: &str,
        params: &crate::api::v1::reviews::ReviewOpenParams,
    ) -> Result<Value, String> {
        let caller_project = self
            .projects
            .project_id_of(entity_id)
            .ok_or_else(|| format!("unknown workspace_id: {}", params.workspace_id))?;
        self.workspaces
            .get(&params.workspace_id)
            .filter(|workspace| workspace.project_id == caller_project)
            .ok_or_else(|| format!("unknown workspace_id: {}", params.workspace_id))?;
        self.review_open(
            params.clone(),
            Actor::Agent {
                agent_id: agent_id.to_string(),
            },
        )
    }

    fn scoped_review_action(
        &mut self,
        entity_id: &str,
        agent_id: &str,
        task_id: &str,
        action: &BridgeAction,
    ) -> Result<Value, String> {
        let caller_project = self
            .projects
            .project_id_of(entity_id)
            .ok_or_else(|| format!("unknown task_id: {task_id}"))?
            .to_string();
        let (task_project, _) = self.tracker_task(task_id)?;
        if caller_project != task_project {
            return Err(format!("unknown task_id: {task_id}"));
        }
        let actor = Actor::Agent {
            agent_id: agent_id.to_string(),
        };
        match action {
            BridgeAction::TrackerGetReview { task_id } => self.review_get(task_id),
            BridgeAction::TrackerReadReview {
                task_id,
                snapshot_id,
                directory_id,
                mode,
                commit,
                from_snapshot_id,
                limit,
                cursor,
                path,
                paths,
                range,
                patch,
            } => self.review_diff(ReviewDiffParams {
                task_id: task_id.clone(),
                snapshot_id: snapshot_id.clone(),
                directory_id: directory_id.clone(),
                read: ReviewReadRequest {
                    mode: *mode,
                    commit: commit.clone(),
                    from_snapshot_id: from_snapshot_id.clone(),
                    limit: *limit,
                    cursor: cursor.clone(),
                    path: path.clone(),
                    paths: paths.clone(),
                    range: *range,
                    patch: *patch,
                },
            }),
            BridgeAction::TrackerPushReview { params } => self.review_push(params.clone(), actor),
            BridgeAction::TrackerUpdateReviewBase { params } => {
                self.review_update(params.clone(), actor)
            }
            BridgeAction::TrackerMergeReview { params } => self.review_merge(params.clone(), actor),
            BridgeAction::TrackerCloseReview { params } => self.review_close(params.clone(), actor),
            BridgeAction::TrackerReopenReview { params } => {
                self.review_reopen(params.clone(), actor)
            }
            BridgeAction::TrackerRetryReviewDelivery { params } => {
                self.review_retry_verdict_delivery(params.clone(), actor)
            }
            BridgeAction::TrackerRefreshReview { params } => {
                self.review_refresh(params.clone(), actor)
            }
            _ => unreachable!("review_task_id selected only review actions"),
        }
    }
}

fn review_task_id(action: &BridgeAction) -> Option<&str> {
    match action {
        BridgeAction::TrackerGetReview { task_id }
        | BridgeAction::TrackerReadReview { task_id, .. }
        | BridgeAction::TrackerPushReview {
            params: crate::api::v1::reviews::ReviewPushParams { task_id, .. },
        }
        | BridgeAction::TrackerUpdateReviewBase {
            params: crate::api::v1::reviews::ReviewUpdateParams { task_id, .. },
        }
        | BridgeAction::TrackerMergeReview {
            params: crate::api::v1::reviews::ReviewMergeParams { task_id, .. },
        }
        | BridgeAction::TrackerCloseReview {
            params: crate::api::v1::reviews::ReviewCloseParams { task_id, .. },
        }
        | BridgeAction::TrackerReopenReview {
            params: crate::api::v1::reviews::ReviewVersionParams { task_id, .. },
        }
        | BridgeAction::TrackerRefreshReview {
            params: crate::api::v1::reviews::ReviewVersionParams { task_id, .. },
        }
        | BridgeAction::TrackerRetryReviewDelivery {
            params: crate::api::v1::reviews::ReviewRetryDeliveryParams { task_id, .. },
        } => Some(task_id),
        _ => None,
    }
}
