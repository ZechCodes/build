use super::*;
use crate::reviews::model::{ReviewMembership, ReviewMembershipKind, ReviewOpeningRequest};
use crate::reviews::opening::{open, OpenReviewRequest};
use std::collections::BTreeMap;

fn opened(state: &mut AppState, project_id: &str, workspace_id: &str) -> String {
    let workspace = state.workspaces.get(workspace_id).unwrap().clone();
    let directory = &workspace.directories[0];
    let request = OpenReviewRequest {
        project_path: state
            .projects
            .get(project_id)
            .unwrap()
            .repo_path
            .to_string_lossy()
            .into(),
        request_id: "summary-opening".into(),
        receiver_root: workspace.root.parent().unwrap().join("receivers"),
        request: ReviewOpeningRequest {
            workspace_id: workspace.id.clone(),
            title: "Review discovery".into(),
            description: "Show the same durable review link on every discovery surface".into(),
            creator: crate::tracker::Actor::User,
            reviewer: None,
            directories: vec![ReviewMembership {
                directory_id: directory.id.clone(),
                source_id: directory.source_id.clone(),
                kind: ReviewMembershipKind::Git,
                reason: None,
            }],
            base_branches: BTreeMap::from([(directory.id.clone(), "refs/heads/main".into())]),
        },
        workspace,
    };
    let opened = open(
        state.tracker_store().unwrap(),
        &request,
        &crate::reviews::sync::reconcile::tests::Hooks,
    )
    .unwrap();
    state.workspaces.reload().unwrap();
    opened.task.id
}

fn assert_discovery(
    state: &mut AppState,
    project: &str,
    workspace: &str,
    task: &str,
    status: &str,
) {
    let summary = serde_json::to_value(
        state
            .tracker_store()
            .unwrap()
            .load_review_summary(task)
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(summary["status"], status);
    let detail = review_call(state, "tasks.get", json!({"task_id": task}));
    assert_eq!(detail["task"]["review_summary"], summary, "{detail}");
    let tasks = review_call(state, "tasks.list", json!({"project_id": project}));
    let row = tasks["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == task)
        .unwrap();
    assert_eq!(row["review_summary"], summary);
    let board = review_call(state, "board.list", json!({}));
    let row = board["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["task_id"] == task)
        .unwrap();
    assert_eq!(row["review_summary"], summary);
    let detail = review_call(state, "workspace.get", json!({"workspace_id": workspace}));
    assert_eq!(detail["active_review"], summary);
    let rows = review_call(state, "workspace.list", json!({"project_id": project}));
    let row = rows["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == workspace)
        .unwrap();
    assert_eq!(row["active_review"], summary);
}

#[test]
fn pr_summary_discovery_preserves_active_and_retained_terminal_links() {
    let tmp = tempfile::tempdir().unwrap();
    let (_repo, mut state, project) = tracked(tmp.path());
    let workspace_id = workspace(&mut state, &project, "summary");
    let task = opened(&mut state, &project, &workspace_id);
    review_call(&mut state, "tasks.watch", json!({"task_id": task}));
    assert_discovery(&mut state, &project, &workspace_id, &task, "open");
    review_call(&mut state, "tasks.close", json!({"task_id": task}));
    assert_discovery(&mut state, &project, &workspace_id, &task, "closed");
    assert!(state.workspaces.get(&workspace_id).is_some());
}

#[test]
fn legacy_task_and_workspace_reads_omit_pr_summary_fields() {
    let tmp = tempfile::tempdir().unwrap();
    let (_repo, mut state, project) = tracked(tmp.path());
    let workspace_id = workspace(&mut state, &project, "legacy-summary");
    let task = filed(&mut state, &project, "Legacy snapshot review");
    review_call(
        &mut state,
        "tasks.review.snapshot",
        json!({
            "task_id": task["id"], "workspace_id": workspace_id, "expected_version": 0,
        }),
    );
    let detail = review_call(&mut state, "tasks.get", json!({"task_id": task["id"]}));
    assert!(detail["task"].get("review_summary").is_none());
    let tasks = review_call(&mut state, "tasks.list", json!({"project_id": project}));
    assert!(tasks["tasks"][0].get("review_summary").is_none());
    let board = review_call(&mut state, "board.list", json!({}));
    let row = board["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["task_id"] == task["id"])
        .unwrap();
    assert!(row.get("review_summary").is_none());
    let detail = review_call(
        &mut state,
        "workspace.get",
        json!({"workspace_id": workspace_id}),
    );
    assert!(detail.get("active_review").is_none());
    let list = review_call(&mut state, "workspace.list", json!({"project_id": project}));
    assert!(list["workspaces"][0].get("active_review").is_none());
}
