//! Review RPCs share the same service and authorization as agent tools.

use super::project_agent::{added_project, project_agent, workspace};
use super::tracker::{filed, tracked};
use super::*;

mod for_task_invalidation;
mod legacy_workspaces;
mod lifecycle_observations;
mod maintenance_observations;
mod merge_contracts;
mod merge_recovery;
mod merge_selection;
mod observation_holds;
mod opening;
mod opening_observations;
mod pull_requests;
#[path = "reviews/pull_requests/reopen_version.rs"]
mod reopen_version;
mod revision_dispatch;
mod summaries;
mod verdict_delivery;
mod verdict_loop;
mod wire_actions;

fn review_call(state: &mut AppState, method: &str, params: Value) -> Value {
    let answer = state.handle(req(method, params));
    assert_eq!(answer["ok"], true, "{answer}");
    answer["result"].clone()
}

/// Seed retained data through the capture service, bypassing retired wire writes.
pub(crate) fn legacy_review(state: &mut AppState, params: Value) -> Value {
    let params: crate::api::v1::reviews::ReviewSnapshotParams =
        serde_json::from_value(params).unwrap();
    let workspace = state.workspaces.get(&params.workspace_id).unwrap().clone();
    let review = crate::reviews::service::snapshot(
        state.tracker_store().unwrap(),
        &crate::reviews::service::SnapshotRequest {
            task_id: params.task_id,
            workspace,
            expected_version: params.expected_version,
            base_overrides: params.base_overrides,
            author: crate::tracker::Actor::User,
        },
    )
    .unwrap();
    json!({"review":review})
}

pub(crate) fn legacy_review_reply(state: &mut AppState, params: Value) -> Value {
    json!({"ok":true,"result":legacy_review(state,params)})
}

#[test]
fn retired_snapshot_and_action_writes_refuse_new_and_legacy_reviews() {
    let tmp = tempfile::tempdir().unwrap();
    let (_repo, mut state, project) = tracked(tmp.path());
    let workspace_id = workspace(&mut state, &project, "legacy");
    let task = filed(&mut state, &project, "Retained review");
    for version in [0, 1] {
        let refusal = state.handle(req(
            "tasks.review.snapshot",
            json!({"task_id":task["id"], "workspace_id":workspace_id, "expected_version":version}),
        ));
        assert_eq!(refusal["error_code"], "invalid_params", "{refusal}");
        assert_eq!(
            refusal["details"]["reason"], "snapshot_retired",
            "{refusal}"
        );
        assert_eq!(refusal["details"]["replacement"], "tasks.review.open");
        assert_eq!(
            refusal["details"]["recovery"],
            "Request a review from the workspace (tasks.review.open)"
        );
        assert_eq!(refusal["retryable"], false);
        if version == 0 {
            legacy_review(
                &mut state,
                json!({"task_id":task["id"], "workspace_id":workspace_id,"expected_version":0}),
            );
        }
    }
    let before = review_call(
        &mut state,
        "tasks.review.get",
        json!({"task_id":task["id"]}),
    );
    let snapshot = &before["review"]["snapshots"][0];
    let refused = state.handle(req("tasks.review.act",json!({"task_id":task["id"],"expected_version":1,"snapshot_id":snapshot["id"],"sources":[{"directory_id":snapshot["directories"][0]["id"],"merge":{"branch":"main"}}]})));
    assert_eq!(refused["error_code"], "invalid_params", "{refused}");
    assert_eq!(refused["details"]["reason"], "snapshot_retired");
    assert_eq!(
        review_call(
            &mut state,
            "tasks.review.get",
            json!({"task_id":task["id"]})
        ),
        before
    );
}

#[test]
fn retained_legacy_review_is_readable_and_completion_is_explicit() {
    let tmp = tempfile::tempdir().unwrap();
    let (_repo, mut state, project) = tracked(tmp.path());
    let workspace_id = workspace(&mut state, &project, "review");
    let task = filed(&mut state, &project, "Review this workspace");
    let task_id = task["id"].as_str().unwrap();
    let before = review_call(&mut state, "tasks.review.get", json!({"task_id": task_id}));
    assert!(before["review"].is_null());
    let saved = legacy_review(
        &mut state,
        json!({
            "task_id": task_id, "workspace_id": workspace_id, "expected_version": 0,
        }),
    );
    assert_eq!(saved["review"]["version"], 1);
    assert_eq!(saved["review"]["snapshots"][0]["number"], 1);
    assert_eq!(saved["review"]["snapshots"][0]["author"]["kind"], "user");
    let held = review_call(&mut state, "tasks.get", json!({"task_id": task_id}));
    assert_eq!(held["task"]["status"], "backlog");
    let stale = state.handle(req(
        "tasks.review.complete",
        json!({
            "task_id": task_id, "expected_version": 0, "description": "Checked and pushed",
        }),
    ));
    assert_eq!(stale["error_code"], "stale_version", "{stale}");
    let completed = review_call(
        &mut state,
        "tasks.review.complete",
        json!({
            "task_id": task_id, "expected_version": 1, "description": "Checked and pushed",
        }),
    );
    assert_eq!(completed["review"]["state"], "completed");
    assert_eq!(completed["review"]["version"], 2);
    let held = review_call(&mut state, "tasks.get", json!({"task_id": task_id}));
    assert_eq!(held["task"]["status"], "done");
    assert_eq!(held["task"]["state"], "open");
    let timeline = held["timeline"].as_array().unwrap();
    assert_eq!(timeline[timeline.len() - 2]["kind"], "moved");
    assert_eq!(
        timeline[timeline.len() - 2]["payload"],
        json!({"from": "backlog", "to": "done"})
    );
    assert_eq!(timeline[timeline.len() - 1]["kind"], "review_completed");
    assert!(state.workspaces.get(&workspace_id).is_some());
}

#[test]
fn review_act_rpc_refuses_a_pr_as_nonretryable_invalid_params_without_admission() {
    let fixture = crate::reviews::sync::reconcile::tests::Fixture::new();
    fixture.commit("pr-action.txt");
    fixture.push();
    fixture.sync();
    let mut state = super::project_agent::rooted(fixture._home.path())
        .with_task_store(fixture._home.path().join("db"))
        .unwrap();
    added_project(&mut state, &fixture.source);
    let before = fixture.review();
    let snapshot = before.snapshots.last().unwrap();
    let source = git2::Repository::open(&fixture.source).unwrap();
    let target = source.refname_to_id("refs/heads/main").unwrap();

    let refused = state.handle(req(
        "tasks.review.act",
        json!({
            "task_id": fixture.task_id(), "expected_version": before.version,
            "snapshot_id": snapshot.id,
            "sources": [{"directory_id": snapshot.directories[0].id,
                "merge": {"branch": "main"}}],
        }),
    ));

    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(refused["error_code"], "invalid_params", "{refused}");
    assert_eq!(refused["retryable"], false, "{refused}");
    assert_eq!(
        refused["error"],
        "PR actions must use the PR merge lifecycle service",
    );
    assert_eq!(
        fixture.review(),
        before,
        "no action admission or version write"
    );
    assert_eq!(source.refname_to_id("refs/heads/main").unwrap(), target);
    assert!(!fixture.source.join("pr-action.txt").exists());
}

#[test]
fn legacy_complete_rpc_refuses_pr_mode_without_changing_it() {
    let tmp = tempfile::tempdir().unwrap();
    let (_repo, mut state, project) = tracked(tmp.path());
    let workspace_id = workspace(&mut state, &project, "pr-complete");
    let opened = wire_actions::open(&mut state, &workspace_id);
    let refusal = state.handle(req("tasks.review.complete",json!({"task_id":opened["task"]["id"],"expected_version":opened["review"]["version"],"description":"Close it"})));
    assert_eq!(refusal["error_code"], "invalid_params", "{refusal}");
    assert_eq!(refusal["details"]["replacement"], "tasks.review.close");
    let current = review_call(
        &mut state,
        "tasks.review.get",
        json!({"task_id":opened["task"]["id"]}),
    );
    assert_eq!(current["review"]["state"], "open");
    assert_eq!(current["review"]["version"], opened["review"]["version"]);
}

#[test]
fn review_comment_metadata_round_trips_and_rejects_foreign_context() {
    let tmp = tempfile::tempdir().unwrap();
    let (_repo, mut state, project) = tracked(tmp.path());
    let workspace_id = workspace(&mut state, &project, "comments");
    let task = filed(&mut state, &project, "Comment on review");
    let task_id = task["id"].as_str().unwrap();
    let saved = legacy_review(
        &mut state,
        json!({
            "task_id": task_id, "workspace_id": workspace_id, "expected_version": 0,
        }),
    );
    let snapshot = &saved["review"]["snapshots"][0];
    let anchor = json!({
        "snapshot_id": snapshot["id"], "directory_id": snapshot["directories"][0]["id"],
        "path": "README.md", "side": "new", "line": 1,
    });
    let first = review_call(
        &mut state,
        "tasks.comment",
        json!({
            "task_id": task_id, "body": "Check this line", "anchor": anchor,
            "opinion": {"snapshot_id": snapshot["id"], "verdict": "request_changes"},
        }),
    );
    assert_eq!(first["comment"]["anchor"], anchor);
    let mut unknown_anchor = anchor.clone();
    unknown_anchor["future"] = json!("unknown");
    let strict_anchor = state.handle(req(
        "tasks.comment",
        json!({
            "task_id": task_id, "body": "Unknown anchor field", "anchor": unknown_anchor,
        }),
    ));
    assert_eq!(
        strict_anchor["error_code"], "invalid_params",
        "{strict_anchor}"
    );
    let strict_opinion = state.handle(req(
        "tasks.comment",
        json!({
            "task_id": task_id, "body": "Unknown opinion field",
            "opinion": {"snapshot_id": snapshot["id"], "verdict": "approve", "future": "unknown"},
        }),
    ));
    assert_eq!(
        strict_opinion["error_code"], "invalid_params",
        "{strict_opinion}"
    );
    let persisted = state
        .tracker_store()
        .unwrap()
        .load_tracker_comment(first["comment"]["id"].as_str().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(persisted.anchor.unwrap().path, "README.md");
    let second = review_call(
        &mut state,
        "tasks.comment",
        json!({
            "task_id": task_id, "body": "Agreed", "reply_to": first["comment"]["id"],
        }),
    );
    assert_eq!(second["comment"]["reply_to"], first["comment"]["id"]);
    let invalid = state.handle(req(
        "tasks.comment",
        json!({
            "task_id": task_id, "body": "Wrong", "anchor": {"snapshot_id": snapshot["id"],
            "directory_id": "other", "path": "README.md", "side": "new", "line": 1},
        }),
    ));
    assert_eq!(invalid["ok"], false, "{invalid}");
    assert_eq!(invalid["error_code"], "invalid_params", "{invalid}");
    let invalid_path = state.handle(req(
        "tasks.comment",
        json!({
            "task_id": task_id, "body": "Wrong path", "anchor": {
                "snapshot_id": snapshot["id"], "directory_id": snapshot["directories"][0]["id"],
                "path": "../README.md", "side": "new", "line": 1,
            },
        }),
    ));
    assert_eq!(invalid_path["ok"], false, "{invalid_path}");
    assert_eq!(
        invalid_path["error_code"], "invalid_params",
        "{invalid_path}"
    );
    let foreign_task = filed(&mut state, &project, "Another task");
    let foreign_reply = state.handle(req(
        "tasks.comment",
        json!({
            "task_id": foreign_task["id"], "body": "Wrong task",
            "reply_to": first["comment"]["id"],
        }),
    ));
    assert_eq!(foreign_reply["ok"], false, "{foreign_reply}");
    assert_eq!(
        foreign_reply["error_code"], "invalid_params",
        "{foreign_reply}"
    );
    let empty_reply = state.handle(req(
        "tasks.comment",
        json!({
            "task_id": task_id, "body": "Wrong reply", "reply_to": "",
        }),
    ));
    assert_eq!(empty_reply["error_code"], "invalid_params", "{empty_reply}");
    let second_snapshot = legacy_review(
        &mut state,
        json!({
            "task_id": task_id, "workspace_id": workspace_id, "expected_version": 1,
        }),
    );
    let new_snapshot_id = &second_snapshot["review"]["snapshots"][1]["id"];
    let mismatched_opinion = state.handle(req(
        "tasks.comment",
        json!({
            "task_id": task_id, "body": "Wrong opinion", "anchor": anchor,
            "opinion": {"snapshot_id": new_snapshot_id, "verdict": "approve"},
        }),
    ));
    assert_eq!(
        mismatched_opinion["error_code"], "invalid_params",
        "{mismatched_opinion}"
    );
    let replacement_workspace = workspace(&mut state, &project, "replacement");
    legacy_review(
        &mut state,
        json!({
            "task_id": task_id, "workspace_id": replacement_workspace, "expected_version": 2,
        }),
    );
    let timeline = review_call(&mut state, "tasks.get", json!({"task_id": task_id}));
    assert!(timeline["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| {
            entry["id"] == first["comment"]["id"]
                && entry["opinion"]["verdict"] == "request_changes"
                && entry["anchor"] == anchor
        }));
}

#[test]
fn mcp_review_comment_mention_watches_the_task_and_uses_the_same_metadata_writer() {
    let tmp = tempfile::tempdir().unwrap();
    let (_repo, mut state, project) = tracked(tmp.path());
    let workspace_id = workspace(&mut state, &project, "agent comments");
    let task = filed(&mut state, &project, "Ask for review");
    let unwatched = review_call(
        &mut state,
        "tasks.unwatch",
        json!({ "task_id": task["id"] }),
    );
    assert_ne!(unwatched["task"]["watched"], true);
    let (owner, agent) = project_agent(&mut state, &project);
    let saved = legacy_review(
        &mut state,
        json!({
            "task_id": task["id"], "workspace_id": workspace_id, "expected_version": 0,
        }),
    );
    let snapshot = &saved["review"]["snapshots"][0];
    let before = review_call(&mut state, "tasks.get", json!({ "task_id": task["id"] }));
    assert_ne!(before["task"]["watched"], true);
    let frame = json!({"jsonrpc":"2.0", "id": 1, "method":"tools/call",
    "params": {"name":"comment_task", "arguments": {
        "task_id": task["id"], "body": "Approved",
        "mention_user": true, "notify_user": false,
        "opinion": {"snapshot_id": snapshot["id"], "verdict":"approve"}
    }}});
    let action = crate::mcp::DoneServer::new(&agent)
        .handle_message(&frame.to_string())
        .action
        .unwrap();
    assert!(matches!(
        action,
        crate::mcp::BridgeAction::TrackerReviewCommentTask {
            mention_user: Some(true),
            notify_user: Some(false),
            ..
        }
    ));
    let result = state.agent_action(&owner, &agent, action).unwrap();
    assert_eq!(result["comment"]["opinion"]["verdict"], "approve");
    assert_eq!(result["comment"]["author"]["agent_id"], agent);
    assert_eq!(result["comment"]["mentions_user"], true);
    assert_eq!(result["task"]["watched"], true);
    let fetched = review_call(&mut state, "tasks.get", json!({ "task_id": task["id"] }));
    assert_eq!(fetched["task"]["watched"], true);
    assert!(fetched["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| { entry["id"] == result["comment"]["id"] && entry["mentions_user"] == true }));
}

#[test]
fn review_anchor_path_is_bounded_in_utf8_bytes() {
    let tmp = tempfile::tempdir().unwrap();
    let (_repo, mut state, project) = tracked(tmp.path());
    let workspace_id = workspace(&mut state, &project, "path bound");
    let task = filed(&mut state, &project, "Review path bound");
    let saved = legacy_review(
        &mut state,
        json!({
            "task_id": task["id"], "workspace_id": workspace_id, "expected_version": 0,
        }),
    );
    let snapshot = &saved["review"]["snapshots"][0];
    let anchor = |path: String| {
        json!({
            "snapshot_id": snapshot["id"], "directory_id": snapshot["directories"][0]["id"],
            "path": path, "side": "new", "line": 1,
        })
    };
    let at_limit = state.handle(req(
        "tasks.comment",
        json!({
            "task_id": task["id"], "body": "At limit", "anchor": anchor("é".repeat(2048)),
        }),
    ));
    assert_eq!(at_limit["ok"], true, "{at_limit}");
    let over_limit = state.handle(req(
        "tasks.comment",
        json!({
            "task_id": task["id"], "body": "Over limit", "anchor": anchor("é".repeat(2049)),
        }),
    ));
    assert_eq!(over_limit["error_code"], "invalid_params", "{over_limit}");
}

#[test]
fn review_rpc_reads_whole_saved_files_and_refuses_caller_supplied_locations() {
    let tmp = tempfile::tempdir().unwrap();
    let (_repo, mut state, project) = tracked(tmp.path());
    let workspace_id = workspace(&mut state, &project, "files");
    let task = filed(&mut state, &project, "Read unchanged files");
    let saved = legacy_review(
        &mut state,
        json!({
            "task_id": task["id"], "workspace_id": workspace_id, "expected_version": 0,
        }),
    );
    let snapshot = &saved["review"]["snapshots"][0];
    assert_eq!(snapshot["directories"][0]["status"], "git", "{saved}");
    let mut params = json!({
        "task_id": task["id"], "snapshot_id": snapshot["id"],
        "directory_id": snapshot["directories"][0]["id"],
        "mode": "blob", "path": "README.md",
    });
    let checkout = state.workspaces.get(&workspace_id).unwrap().directories[0]
        .path
        .clone();
    std::fs::write(checkout.join("README.md"), "unsaved content").unwrap();
    let file = review_call(&mut state, "tasks.review.diff", params.clone());
    assert_eq!(
        file["content_b64"],
        crate::encoding::b64encode(b"# project\n")
    );
    assert_eq!(file["editable"], false);
    params["repo_path"] = json!(tmp.path());
    let refusal = state.handle(req("tasks.review.diff", params));
    assert_eq!(refusal["error_code"], "invalid_params", "{refusal}");
}

fn agent_review(
    state: &mut AppState,
    owner: &str,
    agent: &str,
    action: Value,
) -> Result<Value, String> {
    let action = serde_json::from_value(action).expect("review MCP action parses");
    state.agent_action(owner, agent, action)
}

#[test]
fn review_mcp_get_fences_task_projects() {
    let tmp = tempfile::tempdir().unwrap();
    let (_repo, mut state, project) = tracked(tmp.path());
    let (owner, agent) = project_agent(&mut state, &project);
    let (_other_home, other_repo) = init_repo();
    let other_project = added_project(&mut state, &other_repo);
    let other_task = filed(&mut state, &other_project, "Another project's task");
    let refused = agent_review(
        &mut state,
        &owner,
        &agent,
        json!({"action":"tracker_get_review","task_id":other_task["id"]}),
    )
    .unwrap_err();
    assert!(refused.starts_with("unknown task_id"), "{refused}");
}
