//! Behavioral RPC evidence for input errors, disjoint streams and review modes.
use super::tracker::tracked;
use super::*;
use crate::tracker::{Actor, Assignee, Task, TaskState};
use std::collections::BTreeSet;
const NOW: &str = "2026-10-01T00:00:00Z";

fn query(app: &mut AppState, project: &str, extra: Value) -> Vec<String> {
    let mut params = json!({"project_id":project,"summary":true,"order":"priority"});
    params
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    let response = app.handle(req("tasks.list", params));
    assert_eq!(response["ok"], true, "{response}");
    assert!(
        response["result"].get("next_cursor").is_none(),
        "{response}"
    );
    response["result"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["title"].as_str().unwrap().to_string())
        .collect()
}
fn task(
    app: &AppState,
    project: &str,
    title: &str,
    status: &str,
    closed: bool,
    agent: Option<&str>,
) -> Task {
    let path = app.tracker_project_path(project).unwrap();
    let mut task = Task::drafted(&path, title, Actor::User, NOW);
    task.status = status.into();
    task.priority = crate::tracker::TaskPriority::High;
    task.assignee = agent.map(|id| Assignee::Agent {
        agent_id: id.into(),
    });
    if agent.is_some() {
        task.trackers = vec!["AgentA".into()];
    }
    if closed {
        task.state = TaskState::Closed;
    }
    app.tracker_store()
        .unwrap()
        .create_tracker_task(task, &[])
        .unwrap()
}
fn edge(app: &AppState, project: &str, dependent: &Task, blocker: &Task) {
    app.tracker_store()
        .unwrap()
        .edit_waiting_on(
            &app.tracker_project_path(project).unwrap(),
            &dependent.id,
            std::slice::from_ref(&blocker.id),
            &[],
            &Actor::User,
            NOW,
        )
        .unwrap();
}
fn intent(app: &AppState, task: &Task) -> crate::store::DeliveryIntent {
    app.tracker_store()
        .unwrap()
        .accept_delivery_intent(
            task,
            &[],
            crate::store::DeliveryIntent {
                id: String::new(),
                task_id: task.id.clone(),
                project_path: task.project_path.clone(),
                generation: 0,
                target: json!({"kind":"agent","agent_id":"AgentA"}),
                actor: Actor::User,
                sender: None,
                note: None,
                operation_id: String::new(),
                state: String::new(),
                reservation: Value::Null,
                admitted: None,
                error: None,
            },
        )
        .unwrap()
}
#[test]
fn invalid_selector_values_are_nonretryable_invalid_params_rpc_answers() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    for invalid in [
        json!({"attention_rule":"other"}),
        json!({"exclude_review_mode":"other"}),
        json!({"exclude_assignee":{"kind":"agent","agent_id":""}}),
        json!({"exclude_assignee":{"kind":"agent","agent_id":"x".repeat(129)}}),
        json!({"exclude_assignee":{"kind":"user","extra":true}}),
        json!({"exclude_assignee":{"kind":"agent","agent_id":1}}),
    ] {
        let mut params = json!({"project_id":project,"summary":true});
        params
            .as_object_mut()
            .unwrap()
            .extend(invalid.as_object().unwrap().clone());
        let response = app.handle(req("tasks.list", params));
        assert_eq!(response["ok"], false, "{response}");
        assert_eq!(response["error_code"], "invalid_params", "{response}");
        assert_eq!(response["retryable"], false);
        assert!(response.get("result").is_none());
    }
}
#[test]
fn agent_streams_are_disjoint_for_idle_and_working_roster_states() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let blocker = task(&app, &project, "prerequisite", "backlog", false, None);
    let active = task(&app, &project, "active", "backlog", false, Some("AgentA"));
    let waiting = task(&app, &project, "waiting", "backlog", false, Some("AgentA"));
    edge(&app, &project, &waiting, &blocker);
    assert_eq!(intent(&app, &waiting).state, "waiting");
    let failed = task(&app, &project, "failed", "backlog", false, Some("AgentA"));
    let pending = intent(&app, &failed);
    assert_eq!(pending.state, "queued");
    app.tracker_store()
        .unwrap()
        .fail_delivery_intent(&pending.id, pending.generation, "fixture dispatch failure")
        .unwrap();
    let blocked = task(&app, &project, "blocked", "ready", false, Some("AgentA"));
    edge(&app, &project, &blocked, &blocker);
    for no_intent in [&active, &blocked] {
        assert!(app
            .tracker_store()
            .unwrap()
            .current_delivery_intent(&no_intent.id)
            .unwrap()
            .is_none());
    }
    assert_eq!(
        app.tracker_store()
            .unwrap()
            .current_delivery_intent(&failed.id)
            .unwrap()
            .unwrap()
            .state,
        "failed"
    );
    task(&app, &project, "tracked", "done", true, Some("AgentB"));
    task(&app, &project, "review", "in_review", false, Some("AgentA"));
    task(&app, &project, "done", "done", false, Some("AgentA"));
    for (title, status) in [
        ("closed_backlog", "backlog"),
        ("closed_review", "in_review"),
        ("closed_done", "done"),
    ] {
        task(&app, &project, title, status, true, Some("AgentA"));
    }
    // Working is a cached roster observation, not a persisted task status.
    // Exercise the exact static stream requests for both client observations.
    for working in [false, true] {
        let base = json!({"assignee":{"kind":"agent","agent_id":"AgentA"},"state":"open","statuses":["backlog","ready","in_progress"],"blocked":false});
        let mut working_query = base.clone();
        working_query["undelivered"] = json!(false);
        let working_rows = if working {
            query(&mut app, &project, working_query)
        } else {
            vec![]
        };
        let mut assigned_query = base.clone();
        if working {
            assigned_query["undelivered"] = json!(true);
        }
        let assigned = query(&mut app, &project, assigned_query);
        assert_eq!(working_rows, if working { vec!["active"] } else { vec![] });
        assert_eq!(
            assigned,
            if working {
                vec!["failed"]
            } else {
                vec!["failed", "active"]
            }
        );
        let groups = vec![
            working_rows,
            assigned,
            query(
                &mut app,
                &project,
                json!({"assignee":{"kind":"agent","agent_id":"AgentA"},"state":"open","statuses":["backlog","ready","in_progress"],"blocked":true}),
            ),
            query(
                &mut app,
                &project,
                json!({"tracker_agent_id":"AgentA","exclude_assignee":{"kind":"agent","agent_id":"AgentA"}}),
            ),
            query(
                &mut app,
                &project,
                json!({"assignee":{"kind":"agent","agent_id":"AgentA"},"state":"open","status":"in_review"}),
            ),
            query(
                &mut app,
                &project,
                json!({"assignee":{"kind":"agent","agent_id":"AgentA"},"state":"open","status":"done"}),
            ),
            query(
                &mut app,
                &project,
                json!({"assignee":{"kind":"agent","agent_id":"AgentA"},"state":"closed"}),
            ),
        ];
        assert_eq!(groups[2], vec!["blocked", "waiting"]);
        assert_eq!(groups[3], vec!["tracked"]);
        assert_eq!(groups[4], vec!["review"]);
        assert_eq!(groups[5], vec!["done"]);
        assert_eq!(
            groups[6],
            vec!["closed_done", "closed_review", "closed_backlog"]
        );
        assert_disjoint(groups);
    }
}

fn snapshot(id: &str) -> crate::reviews::model::ReviewSnapshot {
    serde_json::from_value(json!({"publication":null,"id":id,"number":0,"author":{"kind":"user"},"created_at":NOW,"directories":[]})).unwrap()
}
fn publish_pull_request(app: &AppState, project: &str) {
    let store = app.tracker_store().unwrap();
    let path = app.tracker_project_path(project).unwrap();
    let request=serde_json::from_value(json!({"workspace_id":"pr-workspace","title":"pull_request","description":"published fixture","creator":{"kind":"user"},"reviewer":null,"directories":[{"directory_id":"d1","source_id":"s1","kind":"git","reason":null}],"base_branches":{"d1":"refs/heads/main"}})).unwrap();
    let mut opening = store
        .reserve_review_opening(&path, "query-pr-mode", request)
        .unwrap();
    opening.bindings.push(serde_json::from_value(json!({"directory_id":"d1","source_id":"s1","repository_id":"r1","working_repository":"/work/repo","source_repository":"/source/repo","original_branch_ref":"refs/heads/feature","initial_head":"abc123","dedicated_branch_ref":"refs/heads/review/1-feature","base_branch_ref":"refs/heads/main","receiving_repository":"/receive/repo.git","receiving_ref":"refs/heads/review/1-feature","remote_name":"build-review","last_received_head":"abc123","preparation":"ready","publication":"published","recovery":null})).unwrap());
    let saved = store
        .save_review_opening(&opening, opening.version)
        .unwrap();
    let snapshot=serde_json::from_value(json!({"publication":null,"id":"query-pr-snapshot","number":0,"author":{"kind":"user"},"created_at":NOW,"directories":[{"id":"d1","source_id":"s1","name":"repo","path":"/work/repo","source_path":"/source/repo","is_git":true,"status":"git","reason":null,"common_git_dir":"/receive/repo.git","branch":"review/1-feature","base":{"kind":"configured","name":"main","oid":"base123"},"head":"abc123","uncommitted_files":null}]})).unwrap();
    store
        .publish_review_opening(&path, "query-pr-mode", saved.version, snapshot, &[])
        .unwrap();
}
#[test]
fn review_mode_exclusions_query_absent_legacy_snapshot_explicit_snapshot_and_pull_request() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    task(&app, &project, "absent", "backlog", false, None);
    let legacy = task(&app, &project, "legacy_snapshot", "backlog", false, None);
    let explicit = task(&app, &project, "explicit_snapshot", "backlog", false, None);
    for (task, id) in [
        (&legacy, "legacy-snapshot"),
        (&explicit, "explicit-snapshot"),
    ] {
        app.tracker_store()
            .unwrap()
            .save_review_snapshot(&task.id, "snapshot-workspace", 0, snapshot(id))
            .unwrap();
    }
    // Snapshot is omitted by canonical serialization. Import an explicit mode
    // through the supported canonical-table import + projection repair surface.
    let connection = rusqlite::Connection::open(root.path().join("store/build.db")).unwrap();
    let omitted: Option<String> = connection
        .query_row(
            "SELECT json_type(record,'$.mode') FROM reviews WHERE task_id=?1",
            [&legacy.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(omitted, None);
    connection
        .execute(
            "UPDATE reviews SET record=json_set(record,'$.mode','snapshot') WHERE task_id=?1",
            [&explicit.id],
        )
        .unwrap();
    app.tracker_store()
        .unwrap()
        .repair_task_summaries(std::slice::from_ref(&explicit.id))
        .unwrap();
    publish_pull_request(&app, &project);
    let all: BTreeSet<_> = query(&mut app, &project, json!({})).into_iter().collect();
    assert_eq!(
        all,
        BTreeSet::from([
            "absent".into(),
            "legacy_snapshot".into(),
            "explicit_snapshot".into(),
            "pull_request".into()
        ])
    );
    let no_pr: BTreeSet<_> = query(
        &mut app,
        &project,
        json!({"exclude_review_mode":"pull_request"}),
    )
    .into_iter()
    .collect();
    assert_eq!(
        no_pr,
        BTreeSet::from([
            "absent".into(),
            "legacy_snapshot".into(),
            "explicit_snapshot".into()
        ])
    );
    let no_snapshot: BTreeSet<_> = query(
        &mut app,
        &project,
        json!({"exclude_review_mode":"snapshot"}),
    )
    .into_iter()
    .collect();
    assert_eq!(
        no_snapshot,
        BTreeSet::from(["absent".into(), "pull_request".into()])
    );
}

fn assert_disjoint(groups: Vec<Vec<String>>) {
    let mut seen = BTreeSet::new();
    for group in groups {
        for title in group {
            assert!(seen.insert(title), "streams overlap");
        }
    }
    assert_eq!(seen.len(), 10);
}
