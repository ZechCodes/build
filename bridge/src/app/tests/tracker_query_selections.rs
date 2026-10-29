//! Exact static streams preserve membership, order and explicit continuation.
use super::tracker::tracked;
use super::*;
fn list(app: &mut AppState, project: &str, extra: Value) -> Value {
    let mut params = json!({"project_id":project,"summary":true});
    params
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    let response = app.handle(req("tasks.list", params));
    assert_eq!(response["ok"], true, "{response}");
    response["result"].clone()
}
fn seed(app: &AppState, project: &str, assigned: bool, priority: &str) -> crate::tracker::Task {
    let path = app.tracker_project_path(project).unwrap();
    let mut task = crate::tracker::Task::drafted(
        &path,
        "query fixture",
        crate::tracker::Actor::User,
        "2026-10-01T00:00:00Z",
    );
    task.priority = serde_json::from_value(json!(priority)).unwrap();
    task.watched = true;
    if assigned {
        task.assignee = Some(crate::tracker::Assignee::Agent {
            agent_id: "AgentA".into(),
        });
    }
    task.trackers = (0..45)
        .map(|n| format!("tracker-{n}"))
        .chain(["AgentA".into()])
        .collect();
    task.links.workspace_ids = (0..25).map(|n| format!("workspace-{n}")).collect();
    app.tracker_store()
        .unwrap()
        .create_tracker_task(task, &[])
        .unwrap()
}
#[test]
fn exact_memberships_survive_display_caps_and_assignment_wins_tracking() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let assigned = seed(&app, &project, true, "urgent");
    let tracked = seed(&app, &project, false, "high");
    let all = list(
        &mut app,
        &project,
        json!({"linked_workspace_id":"workspace-24","tracker_agent_id":"AgentA","order":"priority"}),
    );
    assert_eq!(all["tasks"][0]["id"], assigned.id);
    assert_eq!(all["tasks"][1]["id"], tracked.id);
    for row in all["tasks"].as_array().unwrap() {
        assert_eq!(
            row["matched_memberships"],
            json!({"linked_workspace_id":"workspace-24","tracker_agent_id":"AgentA"})
        );
        assert!(row["links"]["workspace_ids"].as_array().unwrap().len() < 25);
        assert!(row["trackers"].as_array().unwrap().len() < 46);
    }
    let only_tracked = list(
        &mut app,
        &project,
        json!({"tracker_agent_id":"AgentA","exclude_assignee":{"kind":"agent","agent_id":"AgentA"},"order":"priority"}),
    );
    assert_eq!(only_tracked["tasks"].as_array().unwrap().len(), 1);
    assert_eq!(only_tracked["tasks"][0]["id"], tracked.id);
    let assigned_stream = list(
        &mut app,
        &project,
        json!({"assignee":{"kind":"agent","agent_id":"AgentA"},"state":"open","unfinished":true,"blocked":false,"undelivered":false,"held":true}),
    );
    assert_eq!(assigned_stream["tasks"][0]["id"], assigned.id);
    assert!(
        list(&mut app, &project, json!({"tracker_agent_id":"agenta"}))["tasks"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
#[test]
fn ordered_heads_are_global_and_ties_use_descending_number() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let urgent = seed(&app, &project, false, "urgent");
    let high = seed(&app, &project, false, "high");
    let later = seed(&app, &project, false, "high");
    let head = list(&mut app, &project, json!({"order":"priority","limit":1}));
    assert_eq!(head["tasks"][0]["id"], urgent.id);
    let next = list(
        &mut app,
        &project,
        json!({"order":"priority","limit":1,"cursor":head["next_cursor"]}),
    );
    assert_eq!(next["tasks"][0]["id"], later.id);
    let last = list(
        &mut app,
        &project,
        json!({"order":"priority","limit":1,"cursor":next["next_cursor"]}),
    );
    assert_eq!(last["tasks"][0]["id"], high.id);
    let changed=app.handle(req("tasks.list",json!({"project_id":project,"summary":true,"order":"updated","limit":1,"cursor":head["next_cursor"]})));
    assert_eq!(changed["ok"], false);
    seed(&app, &project, false, "high");
    let stale=app.handle(req("tasks.list",json!({"project_id":project,"summary":true,"order":"priority","limit":1,"cursor":head["next_cursor"]})));
    assert_eq!(stale["error_code"], "conflict", "{stale}");
}
#[test]
fn sparse_static_membership_exposes_progress_without_hidden_draining() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let target = seed(&app, &project, true, "high");
    for _ in 0..12 {
        seed(&app, &project, false, "high");
    }
    let mut params =
        json!({"assignee":{"kind":"agent","agent_id":"AgentA"},"unfinished":true,"limit":1});
    let first = list(&mut app, &project, params.clone());
    assert_eq!(first["tasks"], json!([]));
    assert!(first["next_cursor"].is_string());
    let mut page = first;
    let mut found = Vec::new();
    let mut calls = 0;
    loop {
        found.extend(
            page["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row["id"].clone()),
        );
        let Some(cursor) = page.get("next_cursor") else {
            break;
        };
        params["cursor"] = cursor.clone();
        page = list(&mut app, &project, params.clone());
        calls += 1;
        assert!(calls < 10);
    }
    assert_eq!(found, vec![json!(target.id)]);
    assert!(calls >= 3);
}

#[test]
fn exact_attention_requires_source_predicate_and_done_cutoff_is_inclusive() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let mut task = seed(&app, &project, false, "high");
    assert!(
        list(&mut app, &project, json!({"attention_rule":"asked"}))["tasks"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    task.assignee = Some(crate::tracker::Assignee::User);
    app.tracker_store()
        .unwrap()
        .save_tracker_task_activity(&task, &[], &[])
        .unwrap();
    for rule in ["asked", "legacy"] {
        let page = list(&mut app, &project, json!({"attention_rule":rule}));
        assert_eq!(page["tasks"][0]["id"], task.id);
    }
    task.status = "done".into();
    task.done_at = Some("2026-10-01T00:00:00Z".into());
    app.tracker_store()
        .unwrap()
        .save_tracker_task_activity(&task, &[], &[])
        .unwrap();
    assert!(
        list(&mut app, &project, json!({"attention_rule":"asked"}))["tasks"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let exact = list(&mut app, &project, json!({"done_since":1790812800000_i64}));
    assert_eq!(exact["tasks"][0]["id"], task.id);
    assert!(
        list(&mut app, &project, json!({"done_since":1790812800001_i64}))["tasks"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    task.status = "backlog".into();
    app.tracker_store()
        .unwrap()
        .save_tracker_task_activity(&task, &[], &[])
        .unwrap();
    assert!(list(&mut app, &project, json!({"done_since":0}))["tasks"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn anchor_order_returns_authoritative_fallback_comparator_only_in_its_query() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let first = seed(&app, &project, false, "high");
    let mut later = seed(&app, &project, false, "high");
    later.created_at = "2026-10-02T00:00:00Z".into();
    later.updated_at = later.created_at.clone();
    app.tracker_store()
        .unwrap()
        .save_tracker_task_activity(&later, &[], &[])
        .unwrap();
    let mut absent = seed(&app, &project, false, "high");
    absent.created_at = "invalid".into();
    absent.updated_at = "invalid".into();
    app.tracker_store()
        .unwrap()
        .save_tracker_task_activity(&absent, &[], &[])
        .unwrap();
    let ordered = list(&mut app, &project, json!({"order":"anchor"}));
    assert_eq!(ordered["tasks"][0]["id"], first.id);
    assert_eq!(ordered["tasks"][0]["order_anchor_ms"], 1790812800000_i64);
    assert_eq!(ordered["tasks"][1]["id"], later.id);
    assert_eq!(ordered["tasks"][1]["order_anchor_ms"], 1790899200000_i64);
    assert_eq!(ordered["tasks"][2]["id"], absent.id);
    assert!(ordered["tasks"][2]["order_anchor_ms"].is_null());
    assert!(
        list(&mut app, &project, json!({"order":"number"}))["tasks"][0]
            .get("order_anchor_ms")
            .is_none()
    );
}
