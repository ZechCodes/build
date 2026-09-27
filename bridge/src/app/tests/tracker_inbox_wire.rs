//! The wire under the inbox's watched tasks (#125).
//!
//! An agent's real MCP `tools/call` changes a watched task; the SPA's inbox
//! subscription (`s-inbox`: every entity, `state`/`thread`/`tasks`) is
//! flushed; and the encrypted pushes it was sent are decrypted, beside what
//! `tasks.list` and `tasks.get` answer after each step. The SPA wiring test
//! (spa/test/watchedTaskInboxWire.test.js) runs this and replays exactly that
//! through its real subscriptions, cache and rail.

use super::*;
use crate::mcp::DoneServer;

const INBOX_SUBSCRIPTION: &str = "s-inbox";

/// One agent `tools/call` over the MCP stdio server, answered by the daemon
/// the way a harness's call is.
fn mcp_call(state: &Arc<Mutex<AppState>>, entity: &str, agent: &str, tool: &str, arguments: Value) {
    let frame = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": tool, "arguments": arguments }
    });
    let mut output = Vec::new();
    DoneServer::for_owner(agent)
        .run_stdio(
            std::io::Cursor::new(format!("{frame}\n")),
            &mut output,
            |_| {},
            |action| {
                state
                    .lock()
                    .unwrap()
                    .on_agent_mcp_action(entity, agent, action)
            },
        )
        .expect("the MCP stdio tool call replies");
    let reply: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(reply["result"]["isError"], false, "{tool}: {reply:?}");
}

struct Reader {
    handler: crate::carrier::FrameHandler,
    session: SessionSender,
    rx: tokio::sync::mpsc::UnboundedReceiver<crate::carrier::OutboundEnvelope>,
    key: String,
    changes: Arc<crate::changes::ChangeBus>,
    project: String,
}

impl Reader {
    fn call(&self, method: &str, params: Value) -> Value {
        let answer = self.handler.call(self.session.clone(), req(method, params));
        assert_eq!(answer["ok"], true, "{method}: {answer:?}");
        answer["result"].clone()
    }

    /// Flush the real change window and take every push it sent this session.
    fn pushes(&mut self) -> Vec<Value> {
        self.changes.flush();
        let mut events = Vec::new();
        while let Ok(envelope) = self.rx.try_recv() {
            events.push(SessionSender::decrypt_push(&self.key, &envelope));
        }
        events
    }

    /// What the SPA reads after a push: the project's whole list, and the
    /// task's own record.
    fn step(&mut self, label: &str, task_id: &str) -> Value {
        let events = self.pushes();
        json!({
            "label": label,
            "events": events,
            "list": self.call("tasks.list", json!({ "project_id": self.project })),
            "get": self.call("tasks.get", json!({ "task_id": task_id })),
        })
    }
}

fn task_status(step: &Value, task_id: &str) -> Value {
    step["list"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == task_id)
        .map(|task| task["status"].clone())
        .unwrap_or(Value::Null)
}

fn pushed_task_ids(step: &Value) -> Vec<String> {
    step["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["type"] == "changes")
        .flat_map(|event| event["items"].as_array().unwrap())
        .flat_map(|item| item["tasks"]["task_ids"].as_array().into_iter().flatten())
        .map(|id| id.as_str().unwrap().to_string())
        .collect()
}

#[test]
fn watched_task_inbox_wire_probe() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path()).with_change_window(Duration::ZERO);
    let board = app.handle(req("board.list", json!({})));
    let project = board["result"]["projects"][0]["project_id"]
        .as_str()
        .unwrap()
        .to_string();
    let workspace = super::project_agent::workspace(&mut app, &project, "inbox wire");
    let ensured = app.handle(req(
        "workspace.ensure_conversation",
        json!({ "workspace_id": workspace }),
    ));
    let entity = ensured["result"]["entity_id"].as_str().unwrap().to_string();
    let added = app
        .agent_add(&json!({ "entity_id": entity, "provider": "pi", "notify_user": false }))
        .unwrap();
    let agent = added["agent"]["id"].as_str().unwrap().to_string();
    // The user files both, so the user watches both.
    let review = super::tracker::filed(&mut app, &project, "Review the inbox rows")["id"]
        .as_str()
        .unwrap()
        .to_string();
    let asked = super::tracker::filed(&mut app, &project, "Answer the agent")["id"]
        .as_str()
        .unwrap()
        .to_string();

    let state = app.shared();
    let changes = state.lock().unwrap().changes();
    let handler = AppState::handler(Arc::clone(&state));
    let (session, rx, key) = SessionSender::observable("inbox-reader");
    let mut reader = Reader {
        handler,
        session,
        rx,
        key,
        changes,
        project: project.clone(),
    };
    let greeting = reader.call("session.hello", json!({ "changes": "subscriptions" }));
    reader.call(
        "changes.subscribe",
        json!({
            "subscription_id": INBOX_SUBSCRIPTION, "scope": { "kind": "all" },
            "kinds": ["state", "thread", "tasks"], "mode": "realtime", "priority": "foreground",
        }),
    );
    let start = reader.step("filed", &review);
    // The comment case starts from the same filed state, with its own task.
    let asked_start = json!({
        "label": "filed", "events": [], "list": start["list"],
        "get": reader.call("tasks.get", json!({ "task_id": asked })),
    });
    for id in [&review, &asked] {
        let listed = start["list"]["tasks"].as_array().unwrap();
        let task = listed.iter().find(|task| &task["id"] == id).unwrap();
        assert_eq!(task["watched"], true, "{task:?}");
    }

    mcp_call(
        &state,
        &entity,
        &agent,
        "move_task",
        json!({ "task_id": review, "status": "in_review" }),
    );
    let in_review = reader.step("in_review", &review);
    // In review is not the user's until the reviewer is the user (#144).
    mcp_call(
        &state,
        &entity,
        &agent,
        "assign_task",
        json!({ "task_id": review, "assignee": { "kind": "user" } }),
    );
    let assigned = reader.step("assigned", &review);
    mcp_call(
        &state,
        &entity,
        &agent,
        "move_task",
        json!({ "task_id": review, "status": "done" }),
    );
    let done = reader.step("done", &review);

    // Agents' own traffic on a watched task, then a question put to the user
    // (#144): only the second asks for them.
    mcp_call(
        &state,
        &entity,
        &agent,
        "comment_task",
        json!({ "task_id": asked, "body": "Rebased on main." }),
    );
    let chatter = reader.step("chatter", &asked);
    mcp_call(
        &state,
        &entity,
        &agent,
        "comment_task",
        json!({ "task_id": asked, "body": "Which name should the row use?", "notify_user": true }),
    );
    let commented = reader.step("commented", &asked);
    let comment_id = commented["get"]["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|entry| entry["type"] == "comment")
        .map(|entry| entry["id"].as_str().unwrap().to_string())
        .expect("the agent's comment is on the timeline");
    reader.call(
        "tasks.read_through",
        json!({ "task_id": asked, "event_id": comment_id }),
    );
    let read = reader.step("read", &asked);

    // A single MCP create with mention_user is already a watched question.
    mcp_call(
        &state,
        &entity,
        &agent,
        "create_task",
        json!({ "title": "Choose the route", "body": "Which route should we take?", "mention_user": true, "notify_user": false }),
    );
    let created_task = reader.call("tasks.list", json!({ "project_id": project }))["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["title"] == "Choose the route")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let created_ask = reader.step("created_ask", &created_task);
    let created_event = &created_ask["get"]["timeline"][0];
    assert_eq!(created_ask["get"]["task"]["watched"], true);
    assert_eq!(created_event["kind"], "created");
    assert_eq!(created_event["mentions_user"], true);
    let created_event_id = created_event["id"].as_str().unwrap().to_string();
    reader.call(
        "tasks.read_through",
        json!({ "task_id": created_task, "event_id": created_event_id }),
    );
    let created_read = reader.step("created_read", &created_task);
    for (step, expected) in [(&created_ask, 1), (&created_read, 0)] {
        assert_eq!(step["get"]["task"]["unread_count"], json!(expected));
        let listed = step["list"]["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|task| task["id"] == created_task)
            .unwrap();
        assert_eq!(listed["unread_count"], json!(expected));
    }

    // What the replay depends on: each step's push names its task on the
    // inbox subscription, and the answers after it say what changed.
    for (step, id) in [
        (&in_review, &review),
        (&assigned, &review),
        (&done, &review),
        (&chatter, &asked),
        (&commented, &asked),
        (&read, &asked),
        (&created_ask, &created_task),
        (&created_read, &created_task),
    ] {
        assert!(
            pushed_task_ids(step).contains(id),
            "{} names {id}: {step:?}",
            step["label"]
        );
    }
    assert_eq!(task_status(&in_review, &review), "in_review");
    assert_eq!(
        commented["get"]["timeline"]
            .as_array()
            .unwrap()
            .iter()
            .rev()
            .find(|entry| entry["type"] == "comment")
            .unwrap()["notifies_user"],
        true
    );
    assert_eq!(task_status(&done, &review), "done");
    assert_eq!(read["get"]["task"]["read_through"], json!(comment_id));

    println!(
        "WATCHED_TASK_INBOX_WIRE={}",
        json!({
            "greeting": greeting,
            "subscription_id": INBOX_SUBSCRIPTION,
            "project_id": project,
            "review": { "task_id": review, "steps": [start, in_review, assigned, done] },
            "comment": { "task_id": asked, "steps": [asked_start, chatter, commented, read] },
            "created_ask": { "task_id": created_task, "steps": [created_ask, created_read] },
        })
    );
}
