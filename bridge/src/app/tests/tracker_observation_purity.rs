use super::tracker::{filed, tracked};
use super::tracker_tools::{call, coding_agent};
use super::*;
use crate::mcp::BridgeAction;
use crate::store::TrackerTaskObservation;

struct Fixture {
    _root: tempfile::TempDir,
    _repository: tempfile::TempDir,
    app: AppState,
    project: String,
    actor: String,
    observation: TrackerTaskObservation,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let state_root = std::fs::canonicalize(root.path()).unwrap();
        let (repository, mut app, project) = tracked(&state_root);
        let actor = coding_agent(&mut app, &project, "writer");
        let task = filed(&mut app, &project, "projection identities");
        call(
            &mut app,
            &actor,
            BridgeAction::TrackerCommentTask {
                task_id: task["id"].as_str().unwrap().into(),
                body: "Anonymous live actor".into(),
                refs: Vec::new(),
                track: Some(false),
                attachments: Vec::new(),
                notify_user: None,
                mention_user: None,
            },
        )
        .unwrap();
        app.initialize_snapshot_projection();
        let path = app.tracker_project_path(&project).unwrap();
        let observation = app
            .tracker_store()
            .unwrap()
            .watched_tracker_task_observations(&path)
            .unwrap()
            .pop()
            .unwrap();
        Self {
            _root: root,
            _repository: repository,
            app,
            project,
            actor: actor.1,
            observation,
        }
    }
}

#[test]
fn task_observation_rendering_uses_captured_paths_for_unnamed_live_actors() {
    let fixture = Fixture::new();
    let ((row, answer), comparisons) = crate::app::workspaces::count_path_comparisons(|| {
        (
            fixture
                .app
                .watched_task_observation_row(&fixture.project, &fixture.observation)
                .unwrap(),
            fixture
                .app
                .observed_task_json(&fixture.project, &fixture.observation),
        )
    });

    assert_eq!(
        comparisons, 0,
        "postcommit projection must not canonicalize paths"
    );
    assert_eq!(row["last_event"]["actor"], "the writer agent");
    assert_eq!(answer["identities"][&fixture.actor]["available"], true);
}

#[test]
fn task_observation_rendering_preserves_retired_names_without_the_writer() {
    let mut fixture = Fixture::new();
    let retired = "agent-retired-observation";
    fixture.observation.task.body = format!("@agent:{retired}");
    fixture.observation.task.identities.insert(
        retired.into(),
        crate::tracker::TaskAgentIdentity {
            agent_id: retired.into(),
            name: Some("Historian".into()),
            ordinal: Some(2),
            project_role: None,
            workspace_id: None,
            workspace_name: Some("Earlier work".into()),
            provider: None,
            available: false,
        },
    );
    let store = fixture.app.tracker_store().unwrap().clone();
    let writer = store.writer_for_test();
    let (send, receive) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let answer = fixture
            .app
            .observed_task_json(&fixture.project, &fixture.observation);
        send.send(answer).unwrap();
    });
    let answer = receive.recv_timeout(std::time::Duration::from_millis(500));
    drop(writer);
    worker.join().unwrap();
    let answer = answer.expect("owned task observation cannot wait for the Store writer");
    assert_eq!(answer["identities"][retired]["name"], "Historian");
    assert_eq!(answer["identities"][retired]["available"], false);
}

#[test]
fn dependency_edit_captures_a_new_actor_before_publishing_its_reply() {
    let root = tempfile::tempdir().unwrap();
    let (_repository, mut app, project) = tracked(root.path());
    let dependent = filed(&mut app, &project, "dependent");
    let prerequisite = filed(&mut app, &project, "prerequisite");
    for (name, tool) in [
        ("Link author", "link_task"),
        ("Unlink author", "unlink_task"),
    ] {
        let actor = coding_agent(&mut app, &project, name);
        app.set_agent_name(&actor.0, &actor.1, name).unwrap();
        app.initialize_snapshot_projection();
        let action = crate::mcp::DoneServer::new("test")
            .handle_message(
                &json!({
                    "jsonrpc":"2.0", "id":1, "method":"tools/call",
                    "params":{"name":tool, "arguments":{
                        "task_id":dependent["id"], "waiting_on":[prerequisite["id"]]
                    }}
                })
                .to_string(),
            )
            .action
            .unwrap();

        let answer = call(&mut app, &actor, action).unwrap();
        let identity = &answer["task"]["identities"][&actor.1];
        assert_eq!(identity["name"], name);
        assert!(identity["ordinal"].is_number(), "{identity}");
        assert!(identity["provider"].is_string(), "{identity}");
        assert_eq!(identity["available"], true);
        let (_, saved) = app.tracker_task(dependent["id"].as_str().unwrap()).unwrap();
        assert_eq!(saved.identities[&actor.1].name.as_deref(), Some(name));
    }
}
