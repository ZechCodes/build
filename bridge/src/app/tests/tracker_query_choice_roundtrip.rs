//! Choice vocabulary and exact selection share the same canonical membership.
use super::tracker::tracked;
use super::*;
use crate::tracker::{Actor, Assignee, Task};
use std::collections::BTreeSet;
const NOW: &str = "2026-10-01T00:00:00Z";
fn call(app: &mut AppState, method: &str, params: Value) -> Value {
    let reply = app.handle(req(method, params));
    assert_eq!(reply["ok"], true, "{reply}");
    reply["result"].clone()
}
fn seed(app: &AppState, project: &str, agent: Option<&str>, label: &str, status: &str) -> Task {
    let mut task = Task::drafted(
        &app.tracker_project_path(project).unwrap(),
        "choice fixture",
        Actor::User,
        NOW,
    );
    task.assignee = agent.map(|agent_id| Assignee::Agent {
        agent_id: agent_id.into(),
    });
    task.labels = vec![label.into()];
    task.status = status.into();
    app.tracker_store()
        .unwrap()
        .create_tracker_task(task, &[])
        .unwrap()
}
fn choices(app: &mut AppState, project: &str, dimension: &str) -> Vec<Value> {
    let answer = call(
        app,
        "tasks.filterChoices",
        json!({"project_id":project,"dimension":dimension,"limit":100}),
    );
    assert!(answer.get("next_cursor").is_none());
    answer["choices"].as_array().unwrap().clone()
}
fn selected(app: &mut AppState, project: &str, dimension: &str, value: Value) -> BTreeSet<String> {
    let mut params = json!({"project_id":project,"summary":true,"limit":100});
    params[dimension] = value;
    let answer = call(app, "tasks.list", params);
    assert!(answer.get("next_cursor").is_none());
    answer["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap().to_string())
        .collect()
}
fn encoded(values: Vec<Value>) -> BTreeSet<String> {
    values.into_iter().map(|value| value.to_string()).collect()
}
#[test]
fn label_choices_round_trip_nul_suffix_prefix_and_normal_counterparts() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let tasks = [
        ("api", seed(&app, &project, None, "api", "backlog")),
        (
            "api\0suffix",
            seed(&app, &project, None, "api\0suffix", "backlog"),
        ),
        ("\0api", seed(&app, &project, None, "\0api", "backlog")),
    ];
    let observed = choices(&mut app, &project, "label");
    assert_eq!(
        encoded(observed.clone()),
        encoded(tasks.iter().map(|(label, _)| json!(label)).collect())
    );
    for (label, task) in tasks {
        assert!(observed.contains(&json!(label)));
        assert_eq!(
            selected(&mut app, &project, "label", json!(label)),
            BTreeSet::from([task.id])
        );
    }
}
#[test]
fn assignee_choices_round_trip_full_opaque_ids_without_prefix_aliases() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let tasks = [
        (
            "AgentA",
            seed(&app, &project, Some("AgentA"), "api", "backlog"),
        ),
        (
            "AgentA\0suffix",
            seed(&app, &project, Some("AgentA\0suffix"), "api", "backlog"),
        ),
        (
            "\0AgentA",
            seed(&app, &project, Some("\0AgentA"), "api", "backlog"),
        ),
    ];
    let observed = choices(&mut app, &project, "assignee");
    let expected = tasks
        .iter()
        .map(|(agent, _)| json!({"kind":"agent","agent_id":agent}))
        .collect();
    assert_eq!(encoded(observed.clone()), encoded(expected));
    for (agent, task) in tasks {
        let value = json!({"kind":"agent","agent_id":agent});
        assert!(observed.contains(&value));
        assert_eq!(
            selected(&mut app, &project, "assignee", value),
            BTreeSet::from([task.id])
        );
    }
}
#[test]
fn status_choices_keep_existing_normalized_vocabulary_and_validation() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let mut expected = Vec::new();
    for status in ["backlog", "ready", "in_progress", "in_review", "done"] {
        expected.push((status, seed(&app, &project, None, "api", status)));
    }
    seed(&app, &project, None, "api", "done\0suffix");
    seed(
        &app,
        &project,
        None,
        "api",
        &format!("done{}", "x".repeat(129)),
    );
    let observed = choices(&mut app, &project, "status");
    assert_eq!(
        encoded(observed.clone()),
        encoded(expected.iter().map(|(status, _)| json!(status)).collect())
    );
    for (status, task) in expected {
        assert_eq!(
            selected(&mut app, &project, "status", json!(status)),
            BTreeSet::from([task.id])
        );
    }
    for status in [
        "done\0suffix".to_string(),
        format!("done{}", "x".repeat(129)),
    ] {
        let reply = app.handle(req(
            "tasks.list",
            json!({"project_id":project,"summary":true,"status":status}),
        ));
        assert_eq!(reply["error_code"], "invalid_params", "{reply}");
        assert_eq!(reply["retryable"], false);
        assert!(reply.get("result").is_none());
    }
}
fn arm_pending(app: &AppState, pending: &Task) {
    app.tracker_store()
        .unwrap()
        .accept_delivery_intent(
            pending,
            &[],
            crate::store::DeliveryIntent {
                id: String::new(),
                task_id: pending.id.clone(),
                project_path: pending.project_path.clone(),
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
}
fn pending_assignment_transitions(app: &mut AppState, project: &str) -> Task {
    let mut pending = seed(app, project, None, "api", "backlog");
    arm_pending(app, &pending);
    assert_eq!(choices(app, project, "assignee"), vec![json!("any")]);
    assert_eq!(
        selected(app, project, "assignee", json!("any")),
        BTreeSet::from([pending.id.clone()])
    );
    assert!(selected(app, project, "assignee", json!("none")).is_empty());
    let exact = json!({"kind":"agent","agent_id":"AgentA\0suffix"});
    assert!(selected(app, project, "assignee", exact.clone()).is_empty());
    app.tracker_store()
        .unwrap()
        .repair_task_summaries(std::slice::from_ref(&pending.id))
        .unwrap();
    assert_eq!(choices(app, project, "assignee"), vec![json!("any")]);
    pending.assignee = Some(Assignee::Agent {
        agent_id: "AgentA\0suffix".into(),
    });
    app.tracker_store()
        .unwrap()
        .save_tracker_task_activity(&pending, &[], &[])
        .unwrap();
    assert_eq!(choices(app, project, "assignee"), vec![exact.clone()]);
    assert_eq!(
        selected(app, project, "assignee", exact.clone()),
        BTreeSet::from([pending.id.clone()])
    );
    pending.assignee = None;
    app.tracker_store()
        .unwrap()
        .save_tracker_task_activity(&pending, &[], &[])
        .unwrap();
    let path = app.tracker_project_path(project).unwrap();
    app.tracker_store()
        .unwrap()
        .retire_task_deliveries_of_project(&path)
        .unwrap();
    assert_eq!(choices(app, project, "assignee"), vec![json!("none")]);
    assert!(selected(app, project, "assignee", json!("any")).is_empty());
    assert!(selected(app, project, "assignee", exact.clone()).is_empty());
    arm_pending(app, &pending);
    assert_eq!(choices(app, project, "assignee"), vec![json!("any")]);

    pending
}
#[test]
fn pending_only_any_choice_disappears_on_cancel_without_becoming_exact_target_membership() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let pending = pending_assignment_transitions(&mut app, &project);
    let exact = json!({"kind":"agent","agent_id":"AgentA\0suffix"});

    let none = seed(&app, &project, None, "api", "backlog");
    let mut assigned = seed(&app, &project, Some("AgentA\0suffix"), "api", "backlog");
    assert_eq!(
        encoded(choices(&mut app, &project, "assignee")),
        encoded(vec![json!("none"), json!("any"), exact.clone()])
    );
    assert_eq!(
        selected(&mut app, &project, "assignee", json!("none")),
        BTreeSet::from([none.id.clone()])
    );
    assert_eq!(
        selected(&mut app, &project, "assignee", json!("any")),
        BTreeSet::from([pending.id.clone(), assigned.id.clone()])
    );
    assert_eq!(
        selected(&mut app, &project, "assignee", exact.clone()),
        BTreeSet::from([assigned.id.clone()])
    );
    let path = app.tracker_project_path(&project).unwrap();
    app.tracker_store()
        .unwrap()
        .retire_task_deliveries_of_project(&path)
        .unwrap();
    assert_eq!(
        encoded(choices(&mut app, &project, "assignee")),
        encoded(vec![json!("none"), exact.clone()])
    );
    assert_eq!(
        selected(&mut app, &project, "assignee", json!("none")),
        BTreeSet::from([pending.id, none.id])
    );
    assert_eq!(
        selected(&mut app, &project, "assignee", json!("any")),
        BTreeSet::from([assigned.id.clone()])
    );
    assigned.assignee = Some(Assignee::Agent {
        agent_id: "AgentB".into(),
    });
    app.tracker_store()
        .unwrap()
        .save_tracker_task_activity(&assigned, &[], &[])
        .unwrap();
    app.tracker_store()
        .unwrap()
        .repair_task_summaries(std::slice::from_ref(&assigned.id))
        .unwrap();
    let reassigned = json!({"kind":"agent","agent_id":"AgentB"});
    assert_eq!(
        encoded(choices(&mut app, &project, "assignee")),
        encoded(vec![json!("none"), reassigned.clone()])
    );
    assert!(selected(&mut app, &project, "assignee", exact).is_empty());
    assert_eq!(
        selected(&mut app, &project, "assignee", reassigned),
        BTreeSet::from([assigned.id])
    );
}

#[test]
fn maximum_opaque_agent_choice_pages_and_round_trips_without_restricting_ids() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let agents = ["\0".repeat(127), "\0".repeat(128), "zzzz".into()];
    let tasks = agents
        .iter()
        .map(|agent| seed(&app, &project, Some(agent), "api", "backlog"))
        .collect::<Vec<_>>();
    let mut cursor = None;
    let mut observed = Vec::new();
    for index in 0..3 {
        let mut params = json!({"project_id":project,"dimension":"assignee","limit":1});
        if let Some(value) = cursor.take() {
            params["cursor"] = value;
        }
        let result = call(&mut app, "tasks.filterChoices", params);
        let choice = result["choices"][0].clone();
        let agent = choice["agent_id"].as_str().unwrap();
        let encoded = choice.to_string();
        cursor = result.get("next_cursor").cloned();
        let cursor_bytes = cursor.as_ref().and_then(Value::as_str).map_or(0, str::len);
        eprintln!(
            "opaque_choice raw_id_bytes={} key_bytes={} cursor_bytes={cursor_bytes}",
            agent.len(),
            encoded.len()
        );
        assert!(cursor_bytes <= 2048);
        assert_eq!(cursor.is_some(), index < 2);
        observed.push(choice);
    }
    assert_eq!(
        encoded(observed),
        encoded(
            agents
                .iter()
                .map(|agent| json!({"kind":"agent","agent_id":agent}))
                .collect()
        )
    );
    for (agent, task) in agents.iter().zip(tasks) {
        assert_eq!(
            selected(
                &mut app,
                &project,
                "assignee",
                json!({"kind":"agent","agent_id":agent})
            ),
            BTreeSet::from([task.id])
        );
    }
}
