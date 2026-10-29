//! Opaque bounded identifiers and text remain exact across query projections.
use super::tracker::tracked;
use super::*;
use crate::tracker::{Actor, Assignee, Task};
use std::collections::BTreeSet;
const NOW: &str = "2026-10-01T00:00:00Z";
fn answer(app: &mut AppState, method: &str, params: Value) -> Value {
    let response = app.handle(req(method, params));
    assert_eq!(response["ok"], true, "{response}");
    response["result"].clone()
}
fn selected(app: &mut AppState, project: &str, extra: Value) -> Value {
    let mut params = json!({"project_id":project,"summary":true,"order":"priority"});
    params
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    answer(app, "tasks.list", params)
}
fn ids(response: &Value) -> BTreeSet<String> {
    response["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap().to_string())
        .collect()
}
fn create(app: &AppState, project: &str, agent: &str, status: &str, label: &str) -> Task {
    let mut task = Task::drafted(
        &app.tracker_project_path(project).unwrap(),
        agent,
        Actor::User,
        NOW,
    );
    task.assignee = Some(Assignee::Agent {
        agent_id: agent.into(),
    });
    task.status = status.into();
    task.labels = vec![label.into()];
    task.identities.insert(agent.into(),serde_json::from_value(json!({"agent_id":agent,"name":"Canonical identity","ordinal":null,"workspace_id":null,"workspace_name":null,"provider":"pi","available":false})).unwrap());
    task.trackers = vec!["AgentA".into()];
    app.tracker_store()
        .unwrap()
        .create_tracker_task(task, &[])
        .unwrap()
}
fn assert_counts(app: &mut AppState, project: &str, want: [u64; 3]) {
    let counts = answer(
        app,
        "tasks.counts",
        json!({"project_id":project,"agent_ids":["AgentA","AgentA\u{0}suffix","\u{0}AgentA"]}),
    );
    for (agent, count) in ["AgentA", "AgentA\0suffix", "\0AgentA"]
        .into_iter()
        .zip(want)
    {
        let bucket = counts["agents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|bucket| bucket["agent_id"] == agent)
            .unwrap();
        assert_eq!(bucket["unfinished_open_count"], count, "{counts}");
    }
}
fn assert_agent_views(app: &mut AppState, project: &str, tasks: &[Task]) {
    for task in tasks {
        let agent = match task.assignee.as_ref().unwrap() {
            Assignee::Agent { agent_id } => agent_id,
            _ => unreachable!(),
        };
        let page = selected(
            app,
            project,
            json!({"assignee":{"kind":"agent","agent_id":agent}}),
        );
        assert_eq!(ids(&page), BTreeSet::from([task.id.clone()]));
        assert_eq!(page["tasks"][0]["assignee"]["agent_id"], *agent);
        assert_eq!(page["identities"][agent]["agent_id"], *agent, "{page}");
        let lookup = answer(
            app,
            "tasks.lookup",
            json!({"project_id":project,"number":task.number}),
        );
        assert_eq!(lookup["task"]["assignee"]["agent_id"], *agent);
        assert_eq!(lookup["identities"][agent]["agent_id"], *agent, "{lookup}");
        let excluded = selected(
            app,
            project,
            json!({"tracker_agent_id":"AgentA","exclude_assignee":{"kind":"agent","agent_id":agent}}),
        );
        let want: BTreeSet<String> = tasks
            .iter()
            .filter(|other| other.id != task.id)
            .map(|other| other.id.clone())
            .collect();
        assert_eq!(ids(&excluded), want);
    }
}
#[test]
fn nul_prefix_and_suffix_assignees_keep_exact_counts_positive_groups_exclusions_and_repair() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let tasks = [
        create(&app, &project, "AgentA", "backlog", "api"),
        create(&app, &project, "AgentA\0suffix", "backlog", "api"),
        create(&app, &project, "\0AgentA", "backlog", "api"),
    ];
    assert_counts(&mut app, &project, [1, 1, 1]);
    assert_agent_views(&mut app, &project, &tasks);
    app.tracker_store()
        .unwrap()
        .repair_task_summaries(&tasks.iter().map(|task| task.id.clone()).collect::<Vec<_>>())
        .unwrap();
    assert_counts(&mut app, &project, [1, 1, 1]);
    assert_agent_views(&mut app, &project, &tasks);
    let mut reassigned = tasks[0].clone();
    reassigned.assignee = Some(Assignee::Agent {
        agent_id: "AgentA\0suffix".into(),
    });
    app.tracker_store()
        .unwrap()
        .save_tracker_task_activity(&reassigned, &[], &[])
        .unwrap();
    assert_counts(&mut app, &project, [0, 2, 1]);
    assert_eq!(
        ids(&selected(
            &mut app,
            &project,
            json!({"assignee":{"kind":"agent","agent_id":"AgentA\0suffix"}})
        )),
        BTreeSet::from([tasks[0].id.clone(), tasks[1].id.clone()])
    );
    let path = app.tracker_project_path(&project).unwrap();
    app.tracker_store()
        .unwrap()
        .delete_tracker_tasks_of_project(&path, |_| Ok(()))
        .unwrap();
    assert_counts(&mut app, &project, [0, 0, 0]);
}
#[test]
fn status_and_label_nul_suffixes_do_not_join_ordinary_static_groups() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let ordinary = create(&app, &project, "AgentA", "done", "api");
    let opaque = create(&app, &project, "AgentB", "done\0suffix", "api\0suffix");
    assert_eq!(
        ids(&selected(&mut app, &project, json!({"status":"done"}))),
        BTreeSet::from([ordinary.id.clone()])
    );
    assert_eq!(
        ids(&selected(&mut app, &project, json!({"label":"api"}))),
        BTreeSet::from([ordinary.id.clone()])
    );
    assert_eq!(
        ids(&selected(
            &mut app,
            &project,
            json!({"label":"api\0suffix","unfinished":true})
        )),
        BTreeSet::from([opaque.id.clone()])
    );
    let page = selected(&mut app, &project, json!({"unfinished":true}));
    assert_eq!(page["tasks"][0]["status"], "done\0suffix");
    assert_eq!(page["tasks"][0]["labels"], json!(["api\0suffix"]));
    let overlong = create(
        &app,
        &project,
        "AgentC",
        &format!("done{}", "\0".repeat(125)),
        "other",
    );
    let bounded = answer(
        &mut app,
        "tasks.lookup",
        json!({"project_id":project,"number":overlong.number}),
    );
    assert_ne!(bounded["task"]["status"], "done");
    assert!(bounded["task"]["status"].as_str().unwrap().len() <= 128);
    assert_eq!(bounded["task"]["summary_truncated"], true);
    assert_eq!(
        ids(&selected(&mut app, &project, json!({"status":"done"}))),
        BTreeSet::from([ordinary.id])
    );
}

fn marker_refusal(app: &mut AppState, project: &str, operation: &str, extra: Value) {
    let mut params = json!({"project_id":project});
    if operation == "tasks.list" {
        params["summary"] = json!(true);
        params["attention_rule"] = json!("asked");
        params["limit"] = json!(1);
    }
    params
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    let response = app.handle(req(operation, params));
    assert_eq!(response["ok"], false, "{response}");
    assert_eq!(response["error_code"], "unavailable", "{response}");
    assert_eq!(response["retryable"], false);
    assert_eq!(
        response["details"],
        json!({"reason":"task_query_marker_unsupported","scope":"project_markers","project_id":project,"domain":"opaque-marker-v1","max_bytes":128,"operation":operation})
    );
    assert!(response.get("result").is_none());
    assert!(response.get("next_cursor").is_none());
}
#[test]
fn marker_byte_domain_refusals_are_project_wide_nonretryable_and_read_only() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let mut oldest = create(&app, &project, "AgentA", "backlog", "api");
    for _ in 0..12 {
        create(&app, &project, "AgentA", "backlog", "api");
    }
    let valid = format!("{}\0a", "é".repeat(63));
    assert_eq!(valid.len(), 128);
    let invalid = format!("{}x", "é".repeat(64));
    assert_eq!(invalid.len(), 129);
    for field in ["read_through", "watch_started_after", "dismissed_through"] {
        oldest.read_through = None;
        oldest.watch_started_after = None;
        oldest.dismissed_through = None;
        match field {
            "read_through" => oldest.read_through = Some(invalid.clone()),
            "watch_started_after" => oldest.watch_started_after = Some(invalid.clone()),
            _ => oldest.dismissed_through = Some(invalid.clone()),
        };
        app.tracker_store()
            .unwrap()
            .save_tracker_task_activity(&oldest, &[], &[])
            .unwrap();
        let basis = app.tracker_store().unwrap().task_query_basis().unwrap();
        marker_refusal(&mut app, &project, "tasks.counts", json!({}));
        marker_refusal(
            &mut app,
            &project,
            "tasks.list",
            json!({"tracker_agent_id":"absent"}),
        );
        assert_eq!(
            app.tracker_store().unwrap().task_query_basis().unwrap(),
            basis,
            "refusal must not mutate the cache basis"
        );
        let page = selected(&mut app, &project, json!({"order":"number","limit":1}));
        assert_eq!(page["tasks"].as_array().unwrap().len(), 1);
        oldest.read_through = Some(valid.clone());
        oldest.watch_started_after = Some(valid.clone());
        oldest.dismissed_through = Some(valid.clone());
        app.tracker_store()
            .unwrap()
            .save_tracker_task_activity(&oldest, &[], &[])
            .unwrap();
        let counts = answer(&mut app, "tasks.counts", json!({"project_id":project}));
        assert_eq!(
            counts["basis"],
            app.tracker_store().unwrap().task_query_basis().unwrap()
        );
        let lookup = answer(
            &mut app,
            "tasks.lookup",
            json!({"project_id":project,"number":oldest.number}),
        );
        assert_eq!(lookup["task"][field], valid);
        selected(
            &mut app,
            &project,
            json!({"attention_rule":"asked","tracker_agent_id":"absent"}),
        );
    }
}
#[test]
fn watch_floor_missing_and_explicit_empty_remain_distinct_in_actual_counts() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let mut task = create(&app, &project, "AgentA", "backlog", "api");
    task.watched = true;
    let comment = |id: &str| {
        serde_json::from_value::<crate::tracker::TaskComment>(json!({"id":id,"task_id":task.id,"author":{"kind":"agent","agent_id":"author"},"body":"news","created_at":NOW})).unwrap()
    };
    let before = comment("tc-00000000000000000000000001");
    let after = comment("tc-00000000000000000000000003");
    let mut watched = crate::tracker::TaskEvent::new(
        &task.id,
        Actor::User,
        crate::tracker::TaskEventKind::Watched,
        json!({}),
        NOW,
    );
    watched.id = "te-00000000000000000000000002".into();
    app.tracker_store()
        .unwrap()
        .save_tracker_task_activity(&task, &[before, after], &[watched])
        .unwrap();
    for (floor, want) in [(None, 1), (Some(String::new()), 2), (None, 1)] {
        task.watch_started_after = floor;
        app.tracker_store()
            .unwrap()
            .save_tracker_task_activity(&task, &[], &[])
            .unwrap();
        let counts = answer(&mut app, "tasks.counts", json!({"project_id":project}));
        let lookup = answer(
            &mut app,
            "tasks.lookup",
            json!({"project_id":project,"number":task.number}),
        );
        if task.watch_started_after.is_some() {
            assert_eq!(lookup["task"]["watch_started_after"], "");
        } else {
            assert!(lookup["task"]["watch_started_after"].is_null());
        }
        assert_eq!(counts["watched_unread_total"], want, "{counts}");
    }
}
#[test]
fn pending_target_output_preserves_exact_assignee_id_without_inventing_membership() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let task = create(&app, &project, "AgentA\0suffix", "backlog", "api");
    app.tracker_store()
        .unwrap()
        .accept_delivery_intent(
            &task,
            &[],
            crate::store::DeliveryIntent {
                id: String::new(),
                task_id: task.id.clone(),
                project_path: task.project_path.clone(),
                generation: 0,
                target: json!({"kind":"agent","agent_id":"AgentA\0suffix"}),
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
        .unwrap();
    let page = selected(
        &mut app,
        &project,
        json!({"undelivered":true,"assignee":{"kind":"agent","agent_id":"AgentA\0suffix"}}),
    );
    assert_eq!(ids(&page), BTreeSet::from([task.id]));
    assert_eq!(
        page["tasks"][0]["deferred_assignment"]["target"]["agent_id"],
        "AgentA\0suffix"
    );
    assert!(ids(&selected(
        &mut app,
        &project,
        json!({"assignee":{"kind":"agent","agent_id":"AgentA"}})
    ))
    .is_empty());
}

#[test]
fn created_by_and_identity_dictionary_keep_structural_nul_ids_exact() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    for creator in ["AgentA\0suffix", "\0AgentA"] {
        let mut task = Task::drafted(
            &app.tracker_project_path(&project).unwrap(),
            "created",
            Actor::Agent {
                agent_id: creator.into(),
            },
            NOW,
        );
        task.identities.insert(creator.into(),serde_json::from_value(json!({"agent_id":creator,"name":"Creator name","ordinal":null,"workspace_id":null,"workspace_name":null,"provider":"pi","available":false})).unwrap());
        let saved = app
            .tracker_store()
            .unwrap()
            .create_tracker_task(task, &[])
            .unwrap();
        let lookup = answer(
            &mut app,
            "tasks.lookup",
            json!({"project_id":project,"number":saved.number}),
        );
        assert_eq!(lookup["task"]["created_by"]["agent_id"], creator);
        assert_eq!(lookup["identities"][creator]["agent_id"], creator);
    }
}
