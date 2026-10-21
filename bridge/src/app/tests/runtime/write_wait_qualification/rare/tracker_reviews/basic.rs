use super::super::finite_provider::FiniteProvider;
use super::super::{RareFixture, VerifiedStep};
use crate::app::tests::tracker::{filed, tracked};
use crate::app::AppState;
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

struct EndNoticeSessions(Arc<Mutex<AppState>>);

impl Drop for EndNoticeSessions {
    fn drop(&mut self) {
        let sessions: Vec<_> = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .session_registry
            .test_tabs()
            .map(|(_, tab)| Arc::clone(&tab.session))
            .collect();
        for session in sessions {
            session.end();
        }
    }
}

pub(super) async fn execute() -> Vec<VerifiedStep> {
    let root = tempfile::tempdir().unwrap();
    let (_repo_home, mut app, project) = tracked(root.path());
    // Removing the unassigned dependent's last edge sends a real coordinator
    // notice. Enroll its actual cwd before the autonomous service can start.
    let provider = FiniteProvider::install(&app.projects.get(&project).unwrap().repo_path);
    provider.require_enrolled();
    let blocker = filed(&mut app, &project, "Qualification prerequisite");
    // Active cards cannot acquire an unresolved blocker. This separate
    // unassigned backlog task supplies the dependency service precondition.
    let dependent = filed(&mut app, &project, "Qualification dependent")["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut fixture = RareFixture::new("tasks", app);
    let cleanup = EndNoticeSessions(Arc::clone(fixture.state()));
    let task = create(&mut fixture, &project).await;
    update(&mut fixture, &task).await;
    let comment = comment(&mut fixture, &task).await;
    read_through(&mut fixture, &task, &comment).await;
    watching(&mut fixture, &task, false).await;
    watching(&mut fixture, &task, true).await;
    assign(&mut fixture, &task).await;
    attach(&mut fixture, &project).await;
    dependencies(
        &mut fixture,
        &dependent,
        blocker["id"].as_str().unwrap(),
        true,
    )
    .await;
    dependencies(
        &mut fixture,
        &dependent,
        blocker["id"].as_str().unwrap(),
        false,
    )
    .await;
    close(&mut fixture, &task).await;
    reopen(&mut fixture, &task).await;
    let notice = crate::app::tests::wait_for(crate::timing::write_wait::WAIT_BUDGET, || {
        let app = fixture.state().try_lock().ok()?;
        (app.delivery_queue.fixture_is_drained()
            && provider
                .log
                .turns()
                .iter()
                .any(|turn| turn.contains(&dependent)))
        .then_some(())
    })
    .await;
    let steps = fixture.finish().await;
    drop(cleanup);
    assert!(
        notice.is_some(),
        "the real unblocked coordinator notice must finish"
    );
    assert_eq!(provider.opens.load(Ordering::SeqCst), 1);
    assert_eq!(
        provider
            .log
            .turns()
            .iter()
            .filter(|turn| turn.contains(&dependent))
            .count(),
        1
    );
    assert!(provider.log.ended());
    steps
}

fn saved(fixture: &RareFixture, id: &str) -> Value {
    let app = fixture.state().lock().unwrap();
    serde_json::to_value(
        app.tracker_store()
            .unwrap()
            .load_tracker_task(id)
            .unwrap()
            .unwrap(),
    )
    .unwrap()
}

fn timeline(fixture: &RareFixture, id: &str) -> Vec<Value> {
    let app = fixture.state().lock().unwrap();
    app.tracker_store()
        .unwrap()
        .load_tracker_timeline(id)
        .unwrap()
        .into_iter()
        .map(|entry| serde_json::to_value(entry).unwrap())
        .collect()
}

fn assert_fields(task: &Value, expected: Value) {
    for (field, value) in expected.as_object().unwrap() {
        assert_eq!(&task[field], value, "persisted task field {field}");
    }
}

fn assert_event(fixture: &RareFixture, task: &str, kind: &str) -> Value {
    let entries = timeline(fixture, task);
    let event = entries
        .iter()
        .rev()
        .find(|entry| entry["kind"] == kind)
        .unwrap();
    assert_eq!(event["actor"], json!({"kind":"user"}));
    event.clone()
}

fn board_has_task(fixture: &RareFixture, task: &str) -> bool {
    let app = fixture.state().lock().unwrap();
    let view = app.read_view.load();
    view.get("board.list", "").unwrap()["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["kind"] == "tracker_task" && row["task_id"] == task)
}

async fn create(fixture: &mut RareFixture, project: &str) -> String {
    let proof = fixture
        .call(
            "tasks.create",
            json!({"project_id":project,
        "title":"Qualification task", "body":"Original qualification body"}),
        )
        .await;
    let task = proof.reply["result"]["task"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let persisted = saved(fixture, &task);
    assert_fields(
        &persisted,
        json!({"title":"Qualification task", "body":"Original qualification body",
        "state":"open","status":"backlog","watched":true,"assignee":null,"created_by":{"kind":"user"}}),
    );
    assert_eq!(proof.reply["result"]["dispatch"], Value::Null);
    assert_eq!(timeline(fixture, &task).len(), 1);
    assert_event(fixture, &task, "created");
    assert!(board_has_task(fixture, &task));
    fixture
        .verify(proof, json!({"persisted_task":persisted}))
        .await;
    task
}

async fn update(fixture: &mut RareFixture, task: &str) {
    let proof = fixture
        .call(
            "tasks.update",
            json!({"task_id":task,
        "title":"Qualification updated", "body":"Updated qualification body",
        "priority":"high","status":"in_review","labels":["qualification"]}),
        )
        .await;
    let persisted = saved(fixture, task);
    assert_fields(
        &persisted,
        json!({"title":"Qualification updated","body":"Updated qualification body",
        "priority":"high","status":"in_review","labels":["qualification"]}),
    );
    assert_event(fixture, task, "labelled");
    assert_eq!(
        assert_event(fixture, task, "moved")["payload"],
        json!({"from":"backlog","to":"in_review"})
    );
    fixture
        .verify(proof, json!({"persisted_task":persisted}))
        .await;
}

async fn comment(fixture: &mut RareFixture, task: &str) -> String {
    let proof = fixture
        .call(
            "tasks.comment",
            json!({"task_id":task,"body":"Qualification comment"}),
        )
        .await;
    let id = proof.reply["result"]["comment"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let persisted = {
        let app = fixture.state().lock().unwrap();
        serde_json::to_value(
            app.tracker_store()
                .unwrap()
                .load_tracker_comment(&id)
                .unwrap()
                .unwrap(),
        )
        .unwrap()
    };
    assert_fields(
        &persisted,
        json!({"task_id":task,"body":"Qualification comment","author":{"kind":"user"}}),
    );
    assert!(timeline(fixture, task)
        .iter()
        .any(|entry| entry["id"] == id));
    fixture
        .verify(proof, json!({"persisted_comment":persisted}))
        .await;
    id
}

async fn read_through(fixture: &mut RareFixture, task: &str, comment: &str) {
    assert!(saved(fixture, task)["read_through"].is_null());
    let before = timeline(fixture, task);
    let proof = fixture
        .call(
            "tasks.read_through",
            json!({"task_id":task,"event_id":comment}),
        )
        .await;
    let persisted = saved(fixture, task);
    assert_eq!(persisted["read_through"], comment);
    assert_eq!(timeline(fixture, task), before);
    fixture
        .verify(
            proof,
            json!({"persisted_read_through":persisted["read_through"]}),
        )
        .await;
}

fn saved_watched(fixture: &RareFixture, task: &str) -> bool {
    fixture
        .state()
        .lock()
        .unwrap()
        .tracker_store()
        .unwrap()
        .load_tracker_task(task)
        .unwrap()
        .unwrap()
        .watched
}

async fn watching(fixture: &mut RareFixture, task: &str, watched: bool) {
    assert_ne!(saved_watched(fixture, task), watched);
    let (method, kind) = if watched {
        ("tasks.watch", "watched")
    } else {
        ("tasks.unwatch", "unwatched")
    };
    let proof = fixture.call(method, json!({"task_id":task})).await;
    let persisted = saved(fixture, task);
    assert_eq!(saved_watched(fixture, task), watched);
    let event = assert_event(fixture, task, kind);
    if watched {
        assert_eq!(persisted["watch_started_after"], event["id"]);
    } else {
        assert!(persisted["watch_started_after"].is_null());
    }
    assert_eq!(board_has_task(fixture, task), watched);
    fixture
        .verify(proof, json!({"watched":watched,"event":event}))
        .await;
}

async fn assign(fixture: &mut RareFixture, task: &str) {
    assert!(saved(fixture, task)["assignee"].is_null());
    let proof = fixture
        .call(
            "tasks.assign",
            json!({"task_id":task,"assignee":{"kind":"user"}}),
        )
        .await;
    let persisted = saved(fixture, task);
    assert_eq!(persisted["assignee"], json!({"kind":"user"}));
    assert!(proof.reply["result"]["dispatch"].is_null());
    let event = assert_event(fixture, task, "assigned");
    fixture
        .verify(
            proof,
            json!({"assignee":persisted["assignee"],"event":event}),
        )
        .await;
}

async fn attach(fixture: &mut RareFixture, project: &str) {
    let bytes = b"Qualification attachment content\n";
    let proof = fixture
        .call(
            "tasks.attach",
            json!({"project_id":project,
        "filename":"qualification.txt","content_b64":crate::encoding::b64encode(bytes)}),
        )
        .await;
    let attachment = &proof.reply["result"];
    let path = std::path::Path::new(attachment["path"].as_str().unwrap());
    assert_eq!(std::fs::read(path).unwrap(), bytes);
    assert_eq!(attachment["name"], "qualification.txt");
    assert_eq!(attachment["size"], bytes.len());
    let effect = json!({"verified_bytes":bytes.len(),"path":path});
    fixture.verify(proof, effect).await;
}

async fn dependencies(fixture: &mut RareFixture, task: &str, blocker: &str, add: bool) {
    if add {
        assert_fields(
            &saved(fixture, task),
            json!({"state":"open","status":"backlog","assignee":null}),
        );
    }
    let method = if add {
        "tasks.waiting_on.add"
    } else {
        "tasks.waiting_on.remove"
    };
    let proof = fixture
        .call(method, json!({"task_id":task,"waiting_on":[blocker]}))
        .await;
    let summary = {
        let app = fixture.state().lock().unwrap();
        let store = app.tracker_store().unwrap();
        let task = store.load_tracker_task(task).unwrap().unwrap();
        serde_json::to_value(
            store
                .waiting_on_summary(&task.project_path, &task.id)
                .unwrap(),
        )
        .unwrap()
    };
    assert_eq!(summary["blocked"], add);
    let edges = summary["waiting_on"].as_array().unwrap();
    assert_eq!(edges.iter().any(|edge| edge["task_id"] == blocker), add);
    assert_eq!(edges.len(), usize::from(add));
    fixture
        .verify(proof, json!({"persisted_dependencies":summary}))
        .await;
}

async fn close(fixture: &mut RareFixture, task: &str) {
    assert_eq!(saved(fixture, task)["state"], "open");
    let proof = fixture
        .call(
            "tasks.close",
            json!({"task_id":task,"reason":"Qualification closure"}),
        )
        .await;
    let persisted = saved(fixture, task);
    assert_fields(&persisted, json!({"state":"closed","status":"in_review"}));
    assert!(persisted["closed_at"].is_string());
    assert_eq!(
        assert_event(fixture, task, "closed")["payload"],
        json!({"reason":"Qualification closure"})
    );
    fixture
        .verify(proof, json!({"persisted_task":persisted}))
        .await;
}

async fn reopen(fixture: &mut RareFixture, task: &str) {
    assert_eq!(saved(fixture, task)["state"], "closed");
    let proof = fixture.call("tasks.reopen", json!({"task_id":task})).await;
    let persisted = saved(fixture, task);
    assert_fields(
        &persisted,
        json!({"state":"open","closed_at":null,"status":"in_review"}),
    );
    assert_event(fixture, task, "reopened");
    fixture
        .verify(proof, json!({"persisted_task":persisted}))
        .await;
}

#[cfg(test)]
mod controls {
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn rpc_recipe_basic_tracker_control() {
        crate::git_fixture::environment::isolated_git_test!();
        let steps = super::execute().await;
        let observed = steps
            .iter()
            .map(|step| step.method.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        let expected: std::collections::BTreeSet<_> = [
            "tasks.create",
            "tasks.update",
            "tasks.comment",
            "tasks.read_through",
            "tasks.unwatch",
            "tasks.watch",
            "tasks.assign",
            "tasks.attach",
            "tasks.waiting_on.add",
            "tasks.waiting_on.remove",
            "tasks.close",
            "tasks.reopen",
        ]
        .into_iter()
        .collect();
        assert_eq!(observed, expected);
    }
}
