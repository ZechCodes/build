//! MCP page work is bounded before hydration, independent of project size.
use super::*;
use crate::app::tests::tracker::{filed, tracked};

fn list_action(arguments: Value) -> BridgeAction {
    let mut action = json!({"action":"tracker_list_tasks"});
    action
        .as_object_mut()
        .unwrap()
        .extend(arguments.as_object().unwrap().clone());
    serde_json::from_value(action).unwrap()
}

#[test]
fn mcp_task_pages_bound_decoding_hydration_and_serialized_rows() {
    for total in [205, 2000] {
        let root = tempfile::tempdir().unwrap();
        let (_repo, mut app, project) = tracked(root.path());
        let (owner, agent) = super::super::project_agent::project_agent(&mut app, &project);
        let seed = filed(&mut app, &project, "bounded task page");
        let store = app.store.as_ref().unwrap().clone();
        let mut task = store
            .load_tracker_task(seed["id"].as_str().unwrap())
            .unwrap()
            .unwrap();
        task.body = "full task body witness ".repeat(100);
        for number in 1..total {
            task.id = format!("page-work-{number}");
            store.create_tracker_task(task.clone(), &[]).unwrap();
        }
        for (arguments, expected_rows) in [
            (json!({}), 20),
            (json!({"limit":20}), 20),
            (json!({"limit":7}), 7),
            (json!({"label":"missing-label"}), 0),
        ] {
            let limit = arguments["limit"].as_u64().unwrap_or(20);
            let clock = crate::timing::FrameClock::reporting_to(Arc::new(|_| {}));
            let samples = clock.capture_phases();
            let timer = clock.frame("mcp.control");
            timer.sample_tool("list_tasks");
            let answer = timer
                .sample_read_work(|| {
                    app.on_agent_mcp_action(&owner, &agent, list_action(arguments))
                })
                .unwrap();
            let serialized = serde_json::to_string(&answer).unwrap();
            let parsed: Value = serde_json::from_str(&serialized).unwrap();
            assert_eq!(
                parsed["tasks"].as_array().unwrap().len(),
                expected_rows,
                "total={total}"
            );
            assert!(parsed["next_cursor"].is_string());
            if expected_rows > 0 {
                assert!(parsed["tasks"][0]["body"]
                    .as_str()
                    .unwrap()
                    .contains("full task body witness"));
            }
            let captured = samples.snapshot();
            let metrics = &captured["mcp.control:list_tasks"];
            for (phase, expected) in [
                ("entered.task.fetched", limit + 1),
                ("entered.task.decoded", limit),
                ("entered.task.record_decoded", limit),
                ("entered.task.timeline_id", expected_rows as u64),
                ("entered.task.row", expected_rows as u64),
                ("entered.task.summary", expected_rows as u64),
            ] {
                let actual = metrics[phase]["count"].as_u64().unwrap_or(0);
                assert_eq!(actual, expected, "total={total}, {phase}: {captured}");
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_task_page_cursor_keeps_order_across_mid_walk_updates() {
    let fixture = Fixture::new();
    {
        let mut app = fixture.state.lock().unwrap();
        for n in 0..44 {
            filed(&mut app, &fixture.project, &format!("walk task {n}"));
        }
    }
    let mut frame = fixture.frame(list_action(json!({"limit":5})));
    let first = fixture.reply(frame.clone()).await;
    assert_eq!(first["result"]["tasks"].as_array().unwrap().len(), 5);
    let first_number = first["result"]["tasks"][0]["number"].as_u64().unwrap();
    {
        let app = fixture.state.lock().unwrap();
        let store = app.store.as_ref().unwrap();
        let mut changed = store.load_tracker_task(&fixture.task).unwrap().unwrap();
        changed.title = "updated while walking pages".into();
        changed.updated_at = crate::store::now_rfc3339();
        store
            .save_tracker_task_activity(&changed, &[], &[])
            .unwrap();
        let mut draft = changed.clone();
        draft.id = "filed-during-page-walk".into();
        store.create_tracker_task(draft, &[]).unwrap();
    }
    let mut response = first;
    let mut numbers = Vec::new();
    let mut updated = false;
    loop {
        assert_eq!(response["ok"], true, "{response}");
        for task in response["result"]["tasks"].as_array().unwrap() {
            numbers.push(task["number"].as_u64().unwrap());
            updated |= task["id"] == fixture.task && task["title"] == "updated while walking pages";
        }
        let Some(cursor) = response["result"]["next_cursor"].as_str() else {
            break;
        };
        frame["request"]["cursor"] = json!(cursor);
        response = fixture.reply(frame.clone()).await;
    }
    assert_eq!(numbers, (1..=first_number).rev().collect::<Vec<_>>());
    assert!(
        updated,
        "later pages read current task content without changing the keyset"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_task_page_rejects_limits_above_the_measured_maximum() {
    let fixture = Fixture::new();
    let mut frame = fixture.frame(list_action(json!({})));
    for limit in [0, 21, 500] {
        frame["request"]["limit"] = json!(limit);
        let answer = fixture.reply(frame.clone()).await;
        assert_eq!(answer["ok"], false, "limit={limit}: {answer}");
        assert!(answer["error"].as_str().unwrap().contains("1 to 20"));
    }
}
