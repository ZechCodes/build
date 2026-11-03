use super::*;
use crate::app::tests::tracker::{filed, tracked};
use crate::carrier::testing::{client_request, session_init, ChannelWire};
use crate::carrier::{FrameIntake, OutboundEnvelope};

mod thread_revision;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn task_sql_lane_answers_while_app_is_held_and_observes_next_commit() {
    let dir = tempfile::tempdir().unwrap();
    let (_repo, mut app, project_id) = tracked(dir.path());
    let task = filed(&mut app, &project_id, "before");
    let state = app.shared();
    let clock = Arc::clone(&state.lock().unwrap().frame_clock);
    let transport = crate::transport::generate_transport_keypair();
    let intake = FrameIntake::new(AppState::handler(Arc::clone(&state)), transport.clone());
    let mut wire = ChannelWire::open();
    let key = crate::transport::generate_session_key();
    wire.open_session(
        &intake,
        "sql",
        &session_init("sql", &transport.public_key_b64, &key),
    )
    .unwrap();
    assert!(matches!(
        wire.outbound.try_recv(),
        Ok(OutboundEnvelope::SessionAccept { .. })
    ));

    let (held, holding) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let writer = Arc::clone(&state);
    let holder = std::thread::spawn(move || {
        let _app = writer.lock().unwrap();
        held.send(()).unwrap();
        released.recv_timeout(Duration::from_secs(10)).unwrap();
    });
    holding.recv_timeout(Duration::from_secs(5)).unwrap();
    wire.accept(
        &intake,
        client_request(
            &key,
            "sql",
            "data",
            json!({
                "id": 1, "method": "tasks.get", "params": { "task_id": task["id"] }
            }),
        ),
    )
    .await
    .unwrap();
    let answer =
        tokio::time::timeout(Duration::from_millis(500), final_answer(&mut wire, &key)).await;
    release.send(()).unwrap();
    holder.join().unwrap();
    let answer = answer.expect("SQL read must answer before the app mutex is released");
    assert_eq!(answer["result"]["task"]["title"], "before", "{answer}");
    assert_eq!(clock.stats()["methods"]["tasks.get"]["held"]["sum_ms"], 0.0);

    let updated = state
        .lock()
        .unwrap()
        .dispatch(
            "tasks.update",
            &json!({
                "task_id": task["id"], "title": "after"
            }),
        )
        .unwrap();
    assert_eq!(updated["task"]["title"], "after");
    wire.accept(
        &intake,
        client_request(
            &key,
            "sql",
            "data",
            json!({
                "id": 2, "method": "tasks.get", "params": { "task_id": task["id"] }
            }),
        ),
    )
    .await
    .unwrap();
    let answer = tokio::time::timeout(Duration::from_secs(5), final_answer(&mut wire, &key))
        .await
        .unwrap();
    assert_eq!(answer["result"]["task"]["title"], "after", "{answer}");
}

async fn final_answer(wire: &mut ChannelWire, key: &str) -> Value {
    loop {
        let envelope = wire.outbound.recv().await.unwrap();
        let answer = SessionSender::decrypt_push(key, &envelope);
        if answer.get("accepted").is_none() {
            return answer;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scoped_sync_wire_reads_bypass_the_app_lock_and_device_is_plainly_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (_repo, mut app, project_id) = tracked(dir.path());
    let task = filed(&mut app, &project_id, "durable recovery");
    let state = app.shared();
    let transport = crate::transport::generate_transport_keypair();
    let intake = FrameIntake::new(AppState::handler(Arc::clone(&state)), transport.clone());
    let mut wire = ChannelWire::open();
    let key = crate::transport::generate_session_key();
    wire.open_session(
        &intake,
        "sync",
        &session_init("sync", &transport.public_key_b64, &key),
    )
    .unwrap();
    wire.outbound.try_recv().unwrap();

    let (held, holding) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let holder_state = Arc::clone(&state);
    let holder = std::thread::spawn(move || {
        let _app = holder_state.lock().unwrap();
        held.send(()).unwrap();
        released.recv_timeout(Duration::from_secs(10)).unwrap();
    });
    holding.recv_timeout(Duration::from_secs(5)).unwrap();
    wire.accept(
        &intake,
        client_request(
            &key,
            "sync",
            "data",
            json!({
                "id": 1,
                "method": "sync.snapshot",
                "params": {"scope":{"kind":"task","task_id":task["id"]}}
            }),
        ),
    )
    .await
    .unwrap();
    let answer =
        tokio::time::timeout(Duration::from_millis(500), final_answer(&mut wire, &key)).await;
    release.send(()).unwrap();
    holder.join().unwrap();
    let answer = answer.expect("sync snapshot answers before the app mutex is released");
    assert_eq!(answer["ok"], true, "{answer}");
    assert_eq!(answer["result"]["status"], "ok");
    assert_eq!(answer["result"]["scope"]["kind"], "task");
    assert_eq!(answer["result"]["records"][0]["key"], task["id"]);
    let cursor = answer["result"]["cursor"].as_str().unwrap().to_string();

    let updated = state
        .lock()
        .unwrap()
        .dispatch(
            "tasks.update",
            &json!({"task_id":task["id"],"title":"changed after snapshot"}),
        )
        .unwrap();
    assert_eq!(updated["task"]["title"], "changed after snapshot");
    for (id, method, params) in [
        (
            3,
            "sync.head",
            json!({"scope":{"kind":"task","task_id":task["id"]},"cursor":cursor}),
        ),
        (
            4,
            "sync.changes",
            json!({"scope":{"kind":"task","task_id":task["id"]},"cursor":cursor}),
        ),
    ] {
        wire.accept(
            &intake,
            client_request(
                &key,
                "sync",
                "data",
                json!({"id":id,"method":method,"params":params}),
            ),
        )
        .await
        .unwrap();
        let read = tokio::time::timeout(Duration::from_secs(5), final_answer(&mut wire, &key))
            .await
            .unwrap();
        assert_eq!(read["ok"], true, "{method}: {read}");
        assert_eq!(read["result"]["status"], "ok");
        if method == "sync.changes" {
            assert_eq!(
                read["result"]["changes"][0]["row"]["task"]["title"],
                "changed after snapshot"
            );
        }
    }

    wire.accept(
        &intake,
        client_request(
            &key,
            "sync",
            "data",
            json!({
                "id":5,"method":"sync.snapshot",
                "params":{"scope":{"kind":"project_tasks","project_id":project_id}}
            }),
        ),
    )
    .await
    .unwrap();
    let project_snapshot =
        tokio::time::timeout(Duration::from_secs(5), final_answer(&mut wire, &key))
            .await
            .unwrap();
    assert_eq!(project_snapshot["ok"], true, "{project_snapshot}");
    assert_eq!(project_snapshot["result"]["scope"]["kind"], "project_tasks");
    assert_eq!(
        project_snapshot["result"]["scope"]["project_id"],
        project_id
    );
    assert_ne!(project_id, dir.path().display().to_string());
    assert_eq!(project_snapshot["result"]["records"][0]["key"], task["id"]);

    wire.accept(
        &intake,
        client_request(
            &key,
            "sync",
            "data",
            json!({"id":2,"method":"sync.head","params":{"scope":{"kind":"device"}}}),
        ),
    )
    .await
    .unwrap();
    let refused = tokio::time::timeout(Duration::from_secs(5), final_answer(&mut wire, &key))
        .await
        .unwrap();
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(refused["error_code"], "invalid_params");
    assert_eq!(refused["retryable"], false);
    assert!(refused["error"]
        .as_str()
        .unwrap()
        .contains("unknown variant `device`"));

    wire.accept(
        &intake,
        client_request(
            &key,
            "sync",
            "data",
            json!({
                "id":6,"method":"sync.head",
                "params":{"scope":{"kind":"task","task_id":task["id"]},"unbounded":true}
            }),
        ),
    )
    .await
    .unwrap();
    let refused = tokio::time::timeout(Duration::from_secs(5), final_answer(&mut wire, &key))
        .await
        .unwrap();
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(refused["error_code"], "invalid_params");
    assert!(refused["error"].as_str().unwrap().contains("unknown field"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn write_recovery_reads_bypass_held_app_and_writer() {
    let dir = tempfile::tempdir().unwrap();
    let (_repo, app, _project_id) = tracked(dir.path());
    let store = app.store.clone().unwrap();
    let state = app.shared();
    let transport = crate::transport::generate_transport_keypair();
    let intake = FrameIntake::new(AppState::handler(Arc::clone(&state)), transport.clone());
    let mut wire = ChannelWire::open();
    let key = crate::transport::generate_session_key();
    wire.open_session(
        &intake,
        "writes",
        &session_init("writes", &transport.public_key_b64, &key),
    )
    .unwrap();
    wire.outbound.try_recv().unwrap();
    let (held, holding) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        let _app = state.lock().unwrap();
        let _writer = store.writer_for_test();
        held.send(()).unwrap();
        released.recv_timeout(Duration::from_secs(10)).unwrap();
    });
    holding.recv_timeout(Duration::from_secs(5)).unwrap();
    let mut answers = Vec::new();
    for method in ["writes.status", "writes.subscribe"] {
        wire.accept(
            &intake,
            client_request(
                &key,
                "writes",
                "data",
                json!({
                    "id": method, "method": method,
                    "params": {"operation_ids": ["5d9b2df9-cd22-4cc0-9c50-eef3c7c3301a"]}
                }),
            ),
        )
        .await
        .unwrap();
        answers.push(
            tokio::time::timeout(Duration::from_millis(100), final_answer(&mut wire, &key)).await,
        );
    }
    release.send(()).unwrap();
    holder.join().unwrap();
    for answer in answers {
        let answer =
            answer.expect("write recovery reads must finish within100ms with both locks held");
        assert_eq!(answer["ok"], true, "{answer}");
        assert_eq!(
            answer["result"]["operations"][0]["state"], "not_found",
            "{answer}"
        );
        assert!(
            answer["result"]["journal_incarnation"].is_string(),
            "{answer}"
        );
    }
}
