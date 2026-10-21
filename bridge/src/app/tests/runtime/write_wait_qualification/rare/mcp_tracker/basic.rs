use super::{TrackerFixture, VerifiedStep};
use crate::app::tests::project_agent::workspace;
use crate::app::tests::tracker::{filed, tracked};
use serde_json::{json, Value};

pub(super) async fn execute() -> Vec<VerifiedStep> {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let blocker = filed(&mut app, &project, "MCP prerequisite")["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let workspace = workspace(&mut app, &project, "mcp-links");
    let mut fixture = TrackerFixture::new("mcp-tracker", app);
    let task = create(&mut fixture, &blocker).await;
    move_and_label(&mut fixture, &task).await;
    let comment = comment(&mut fixture, &task).await;
    reads(&fixture, &task, &comment, &blocker).await;
    assign_and_track(&mut fixture, &task).await;
    links(&mut fixture, &task, &workspace, &blocker).await;
    retry(&mut fixture, &blocker).await;
    let before = saved(&fixture, &task);
    fixture.wrapped_read_refusal(&task).await;
    assert_eq!(
        saved(&fixture, &task),
        before,
        "wrapped read refusal changes no task"
    );
    close(&mut fixture, &task).await;
    fixture.finish().await
}

fn saved(fixture: &TrackerFixture, task: &str) -> Value {
    serde_json::to_value(
        fixture
            .state()
            .lock()
            .unwrap()
            .tracker_store()
            .unwrap()
            .load_tracker_task(task)
            .unwrap()
            .unwrap(),
    )
    .unwrap()
}

fn event(fixture: &TrackerFixture, task: &str, kind: &str) -> Value {
    let entries = fixture
        .state()
        .lock()
        .unwrap()
        .tracker_store()
        .unwrap()
        .load_tracker_timeline(task)
        .unwrap();
    let event = entries
        .into_iter()
        .map(|entry| serde_json::to_value(entry).unwrap())
        .rev()
        .find(|entry| entry["kind"] == kind)
        .unwrap();
    assert_eq!(event["actor"], fixture.actor());
    event
}

fn dependencies(fixture: &TrackerFixture, task: &str) -> Value {
    let app = fixture.state().lock().unwrap();
    let store = app.tracker_store().unwrap();
    let task = store.load_tracker_task(task).unwrap().unwrap();
    serde_json::to_value(
        store
            .waiting_on_summary(&task.project_path, &task.id)
            .unwrap(),
    )
    .unwrap()
}

async fn create(fixture: &mut TrackerFixture, blocker: &str) -> String {
    let proof = fixture
        .call(
            "create_task",
            json!({"title":"MCP plain task","track":false,
        "notify_user":false,"mention_user":false}),
        )
        .await;
    let id = proof.reply["result"]["task"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let task = saved(fixture, &id);
    assert_eq!(task["title"], "MCP plain task");
    assert_eq!(task["created_by"], fixture.actor());
    fixture.verify(proof, json!({"persisted_task":task})).await;
    wrapped_create(fixture, blocker).await;
    id
}

async fn wrapped_create(fixture: &mut TrackerFixture, blocker: &str) -> String {
    let proof = fixture
        .call(
            "create_task",
            json!({"title":"MCP waiting task","waiting_on":[blocker],
        "track":false,"notify_user":false,"mention_user":false}),
        )
        .await;
    let id = proof.reply["result"]["task"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let task = saved(fixture, &id);
    let summary = dependencies(fixture, &id);
    assert_eq!(task["created_by"], fixture.actor());
    assert_eq!(summary["blocked"], true);
    assert_eq!(summary["waiting_on"][0]["task_id"], blocker);
    fixture
        .verify(proof, json!({"persisted_task":task,"dependencies":summary}))
        .await;
    id
}

async fn move_and_label(fixture: &mut TrackerFixture, task: &str) {
    let proof = fixture
        .call(
            "move_task",
            json!({"task_id":task,"status":"in_progress","track":false}),
        )
        .await;
    assert_eq!(saved(fixture, task)["status"], "in_progress");
    let moved = event(fixture, task, "moved");
    assert_eq!(
        moved["payload"],
        json!({"from":"backlog","to":"in_progress"})
    );
    fixture.verify(proof, json!({"persisted_move":moved})).await;
    let proof = fixture
        .call(
            "label_task",
            json!({"task_id":task,"add":["mcp-qualification"],"remove":[],"track":false}),
        )
        .await;
    assert_eq!(saved(fixture, task)["labels"], json!(["mcp-qualification"]));
    let labelled = event(fixture, task, "labelled");
    fixture
        .verify(proof, json!({"persisted_label":labelled}))
        .await;
}

async fn comment(fixture: &mut TrackerFixture, task: &str) -> String {
    let proof = fixture
        .call(
            "comment_task",
            json!({"task_id":task,"body":"MCP ordinary comment",
        "track":false,"notify_user":false,"mention_user":false}),
        )
        .await;
    let id = proof.reply["result"]["comment"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let stored = fixture
        .state()
        .lock()
        .unwrap()
        .tracker_store()
        .unwrap()
        .load_tracker_comment(&id)
        .unwrap()
        .unwrap();
    assert_eq!(stored.body, "MCP ordinary comment");
    assert_eq!(
        serde_json::to_value(&stored.author).unwrap(),
        fixture.actor()
    );
    fixture
        .verify(proof, json!({"persisted_comment":stored}))
        .await;
    id
}

async fn reads(fixture: &TrackerFixture, task: &str, comment: &str, blocker: &str) {
    let proof = fixture.call("get_task", json!({"task_id":task})).await;
    assert_eq!(proof.reply["result"]["task"]["id"], task);
    let proof = fixture
        .call("list_tasks", json!({"status":"in_progress"}))
        .await;
    assert!(proof.reply["result"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["id"] == task));
    let proof = fixture
        .call("read_comment", json!({"comment_id":comment}))
        .await;
    assert_eq!(proof.reply["result"]["body"], "MCP ordinary comment");
    assert_eq!(proof.reply["result"]["author"], fixture.actor());
    let proof = fixture
        .call(
            "list_task_dependents",
            json!({"task_id":blocker,"limit":10}),
        )
        .await;
    assert_eq!(
        proof.reply["result"]["blocks"][0]["title"],
        "MCP waiting task"
    );
    eprintln!(
        "mcp_tracker_read_effect={}",
        json!({"task":task,"comment":comment,"blocker":blocker,
        "dependents_reply":proof.reply})
    );
}

async fn assign_and_track(fixture: &mut TrackerFixture, task: &str) {
    let proof = fixture
        .call(
            "assign_task",
            json!({"task_id":task,"assignee":{"kind":"user"},
        "track":false,"notify_user":false}),
        )
        .await;
    assert_eq!(saved(fixture, task)["assignee"], json!({"kind":"user"}));
    let assigned = event(fixture, task, "assigned");
    fixture
        .verify(proof, json!({"persisted_assignment":assigned}))
        .await;
    let proof = fixture.call("track_task", json!({"task_id":task})).await;
    assert!(saved(fixture, task)["trackers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|id| id == &fixture.agent));
    let tracked = event(fixture, task, "tracked");
    assert_eq!(tracked["payload"]["agent_id"], fixture.agent);
    fixture
        .verify(proof, json!({"persisted_track":tracked}))
        .await;
    let proof = fixture.call("untrack_task", json!({"task_id":task})).await;
    assert!(!saved(fixture, task)["trackers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|id| id == &fixture.agent));
    let untracked = event(fixture, task, "untracked");
    fixture
        .verify(proof, json!({"persisted_untrack":untracked}))
        .await;
}

async fn links(fixture: &mut TrackerFixture, task: &str, workspace: &str, blocker: &str) {
    let proof = fixture
        .call(
            "link_task",
            json!({"task_id":task,"workspace_id":workspace,"track":false}),
        )
        .await;
    assert!(saved(fixture, task)["links"]["workspace_ids"]
        .as_array()
        .unwrap()
        .iter()
        .any(|id| id == workspace));
    fixture
        .verify(
            proof,
            json!({"persisted_link":event(fixture, task, "linked")}),
        )
        .await;
    let proof = fixture
        .call(
            "link_task",
            json!({"task_id":task,"waiting_on":[blocker],"track":false}),
        )
        .await;
    let summary = dependencies(fixture, task);
    assert_eq!(summary["blocked"], true);
    assert_eq!(summary["waiting_on"][0]["task_id"], blocker);
    fixture
        .verify(proof, json!({"persisted_dependencies":summary}))
        .await;
    let proof = fixture
        .call(
            "unlink_task",
            json!({"task_id":task,"waiting_on":[blocker],"track":false}),
        )
        .await;
    let summary = dependencies(fixture, task);
    assert_eq!(summary["blocked"], false);
    assert!(summary["waiting_on"].as_array().unwrap().is_empty());
    fixture
        .verify(proof, json!({"persisted_dependencies":summary}))
        .await;
}

async fn retry(fixture: &mut TrackerFixture, blocker: &str) {
    let task = wrapped_create(fixture, blocker).await;
    let proof = fixture.call("assign_task", json!({"task_id":task,
        "assignee":{"kind":"new_project_agent","agent_name":"Saved recipient"},"note":"Saved MCP intent",
        "track":false,"notify_user":false})).await;
    let intent = proof.reply["result"]["task"]["deferred_assignment"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let saved = fixture
        .state()
        .lock()
        .unwrap()
        .tracker_store()
        .unwrap()
        .load_delivery_intent(&intent)
        .unwrap()
        .unwrap();
    assert_eq!(saved.state, "waiting");
    fixture
        .verify(
            proof,
            json!({"saved_waiting_intent":intent,"generation":saved.generation}),
        )
        .await;
    fixture
        .state()
        .lock()
        .unwrap()
        .tracker_store()
        .unwrap()
        .fail_delivery_intent(&intent, saved.generation, "fixture failed delivery")
        .unwrap();
    let proof = fixture
        .call(
            "assign_task",
            json!({"task_id":task,"retry_deferred_id":intent,
        "track":false,"notify_user":false}),
        )
        .await;
    let retried = fixture
        .state()
        .lock()
        .unwrap()
        .tracker_store()
        .unwrap()
        .load_delivery_intent(&intent)
        .unwrap()
        .unwrap();
    assert_eq!(retried.state, "waiting");
    assert_eq!(retried.target, saved.target);
    assert_eq!(retried.note, saved.note);
    assert_eq!(retried.actor, saved.actor);
    assert_eq!(retried.sender, saved.sender);
    assert_eq!(retried.generation, saved.generation);
    assert_eq!(retried.operation_id, saved.operation_id);
    fixture
        .verify(
            proof,
            json!({"retried_intent":intent,"state":retried.state,"generation":retried.generation}),
        )
        .await;
}

async fn close(fixture: &mut TrackerFixture, task: &str) {
    let proof = fixture
        .call(
            "close_task",
            json!({"task_id":task,"reason":"MCP qualification close","track":false}),
        )
        .await;
    assert_eq!(saved(fixture, task)["state"], "closed");
    let closed = event(fixture, task, "closed");
    assert_eq!(closed["payload"]["reason"], "MCP qualification close");
    fixture
        .verify(proof, json!({"persisted_close":closed}))
        .await;
}
