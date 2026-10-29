//! Opaque retained navigation references are exact or explicitly refused.
use super::tracker::tracked;
use super::*;
use crate::tracker::{Actor, Assignee, Task, TaskAgentIdentity};

fn identity(agent: &str, workspace: &str) -> TaskAgentIdentity {
    TaskAgentIdentity {
        agent_id: agent.into(),
        name: Some("Retained worker".into()),
        project_role: None,
        ordinal: Some(7),
        workspace_id: Some(workspace.into()),
        workspace_name: Some("Former checkout".into()),
        provider: Some("pi".into()),
        available: false,
    }
}

fn saved_task(app: &AppState, project: &str, workspace: &str, creator: bool) -> Task {
    let path = app.tracker_project_path(project).unwrap();
    let agent = "agent-retained-navigation";
    let mut task = Task::drafted(
        &path,
        "Retained navigation reference",
        if creator {
            Actor::Agent {
                agent_id: agent.into(),
            }
        } else {
            Actor::User
        },
        "2026-10-27T00:00:00Z",
    );
    if !creator {
        task.assignee = Some(Assignee::Agent {
            agent_id: agent.into(),
        });
    }
    task.identities
        .insert(agent.into(), identity(agent, workspace));
    app.tracker_store()
        .unwrap()
        .create_tracker_task(task, &[])
        .unwrap()
}

fn summary(app: &mut AppState, project: &str) -> Value {
    app.handle(req(
        "tasks.list",
        json!({"project_id":project,"summary":true}),
    ))
}

fn assert_stored_data_error(response: &Value) {
    assert_eq!(response["ok"], false, "{response}");
    assert_eq!(response["error_code"], "unavailable", "{response}");
    assert_eq!(response["retryable"], false, "{response}");
    assert!(response["error"]
        .as_str()
        .unwrap()
        .contains("reference exceeds 128 bytes"));
    assert!(
        response.get("result").is_none(),
        "no partial page: {response}"
    );
    assert!(
        response.get("next_cursor").is_none(),
        "no success continuation: {response}"
    );
}

#[test]
fn retained_navigation_ids_over_128_bytes_are_refused_for_both_references() {
    for workspace in [
        "W".repeat(129),
        format!("{}x", "é".repeat(64)),
        "é".repeat(65),
    ] {
        for creator in [true, false] {
            let root = tempfile::tempdir().unwrap();
            let (_repo, mut app, project) = tracked(root.path());
            let task = saved_task(&app, &project, &workspace, creator);
            let legacy = app.handle(req("tasks.get", json!({"task_id":task.id})));
            assert_eq!(legacy["ok"], true, "{legacy}");
            assert_eq!(
                legacy["result"]["task"]["identities"]["agent-retained-navigation"]["workspace_id"],
                workspace
            );
            assert_stored_data_error(&summary(&mut app, &project));
        }
    }
}

#[test]
fn retained_navigation_ids_at_byte_boundary_and_descriptions_remain_exact() {
    for workspace in ["W".repeat(128), "é".repeat(64), "workspace-retained".into()] {
        let root = tempfile::tempdir().unwrap();
        let (_repo, mut app, project) = tracked(root.path());
        let task = saved_task(&app, &project, &workspace, true);
        let response = summary(&mut app, &project);
        assert_eq!(response["ok"], true, "{response}");
        assert_eq!(
            response["result"]["identities"]["agent-retained-navigation"],
            serde_json::to_value(&task.identities["agent-retained-navigation"]).unwrap()
        );
    }
}

#[test]
fn oversized_unreferenced_timeline_identity_does_not_invalidate_card() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let mut task = saved_task(&app, &project, "workspace-valid", true);
    task.identities.insert(
        "agent-history-only".into(),
        identity("agent-history-only", &"W".repeat(129)),
    );
    app.tracker_store()
        .unwrap()
        .save_tracker_task_activity(&task, &[], &[])
        .unwrap();
    let response = summary(&mut app, &project);
    assert_eq!(response["ok"], true, "{response}");
    assert!(response["result"]["identities"]
        .get("agent-history-only")
        .is_none());
    assert_eq!(
        response["result"]["identities"]["agent-retained-navigation"]["workspace_id"],
        "workspace-valid"
    );
}

#[test]
fn repair_and_definition_rebuild_refuse_previously_truncated_navigation_ids() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let task = saved_task(&app, &project, &"W".repeat(129), true);
    let database = root.path().join("store/build.db");
    let connection = rusqlite::Connection::open(database).unwrap();
    // Model the old materialization: a plausible shortened identity and no
    // invalid-reference marker, while canonical data still has all 129 bytes.
    connection.execute("UPDATE tracker_task_summaries SET record=json_set(record,'$._summary_invalid_refs',json('false')), identities=json_set(identities,'$.\"agent-retained-navigation\".workspace_id',?1) WHERE task_id=?2", rusqlite::params!["W".repeat(128), task.id]).unwrap();
    app.tracker_store()
        .unwrap()
        .repair_task_summaries(std::slice::from_ref(&task.id))
        .unwrap();
    assert_stored_data_error(&summary(&mut app, &project));
    connection.execute("UPDATE tracker_task_summaries SET record=json_set(record,'$._summary_invalid_refs',json('false')), identities=json_set(identities,'$.\"agent-retained-navigation\".workspace_id',?1) WHERE task_id=?2", rusqlite::params!["W".repeat(128), task.id]).unwrap();
    connection.execute("UPDATE meta SET value='previous-definition' WHERE key IN ('task_summary_v1_definition','task_summary_v1_ready')", []).unwrap();
    let reopened = crate::store::Store::new(root.path().join("store")).unwrap();
    assert!(matches!(
        reopened.list_task_summary_candidates(&task.project_path, None, 1),
        Err(crate::store::StoreError::TaskSummaryInvalid(_))
    ));
    assert_stored_data_error(&summary(&mut app, &project));
}
