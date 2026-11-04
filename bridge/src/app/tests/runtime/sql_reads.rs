use super::*;
use crate::app::tests::project_agent::{added_project, rooted};
use crate::app::tests::tracker::{filed, tracked};
use crate::carrier::testing::{client_request, session_init, ChannelWire};
use crate::carrier::{FrameIntake, OutboundEnvelope};

mod device_sync;
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
async fn scoped_sync_wire_reads_bypass_the_app_lock_and_device_head_is_available() {
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
    let available = tokio::time::timeout(Duration::from_secs(5), final_answer(&mut wire, &key))
        .await
        .unwrap();
    assert_eq!(available["ok"], true, "{available}");
    assert_eq!(available["result"]["scope"]["kind"], "device");

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
async fn removed_scopes_resync_only_for_authenticated_positions() {
    let dir = tempfile::tempdir().unwrap();
    let (_repo, mut app, project_id) = tracked(dir.path());
    let task = filed(&mut app, &project_id, "removed scope");
    let _second = filed(&mut app, &project_id, "removed scope page two");
    let task_id = task["id"].as_str().unwrap().to_owned();
    let state = app.shared();
    let transport = crate::transport::generate_transport_keypair();
    let intake = FrameIntake::new(AppState::handler(Arc::clone(&state)), transport.clone());
    let mut wire = ChannelWire::open();
    let key = crate::transport::generate_session_key();
    wire.open_session(
        &intake,
        "removed-sync",
        &session_init("removed-sync", &transport.public_key_b64, &key),
    )
    .unwrap();
    wire.outbound.try_recv().unwrap();

    async fn ask(
        wire: &mut ChannelWire,
        intake: &Arc<FrameIntake>,
        key: &str,
        id: u64,
        method: &str,
        params: Value,
    ) -> Value {
        wire.accept(
            intake,
            client_request(
                key,
                "removed-sync",
                "data",
                json!({"id":id,"method":method,"params":params}),
            ),
        )
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(5), final_answer(wire, key))
            .await
            .unwrap()
    }

    let project_scope = json!({"kind":"project_tasks","project_id":project_id});
    let task_scope = json!({"kind":"task","task_id":task_id});
    let project = ask(
        &mut wire,
        &intake,
        &key,
        1,
        "sync.snapshot",
        json!({"scope":project_scope,"limit":1}),
    )
    .await;
    let task = ask(
        &mut wire,
        &intake,
        &key,
        2,
        "sync.snapshot",
        json!({"scope":task_scope}),
    )
    .await;
    let project_cursor = project["result"]["cursor"].as_str().unwrap().to_owned();
    let project_page = project["result"]["next_page"]
        .as_str()
        .expect("two records produce a continuation")
        .to_owned();
    let task_cursor = task["result"]["cursor"].as_str().unwrap().to_owned();

    {
        let mut app = state.lock().unwrap();
        let path = app.tracker_project_path(&project_id).unwrap();
        app.store
            .as_ref()
            .unwrap()
            .delete_tracker_tasks_of_project(&path, |_| Ok(()))
            .unwrap();
        app.projects.remove(&project_id);
        app.publish_read_metadata();
    }

    for (id, scope, cursor) in [
        (3, project_scope.clone(), project_cursor.clone()),
        (4, task_scope.clone(), task_cursor.clone()),
    ] {
        let answer = ask(
            &mut wire,
            &intake,
            &key,
            id,
            "sync.changes",
            json!({"scope":scope,"cursor":cursor}),
        )
        .await;
        assert_eq!(answer["ok"], true, "{answer}");
        assert_eq!(answer["result"]["status"], "resync_required", "{answer}");
        assert_eq!(answer["result"]["reason"], "scope_removed", "{answer}");
        assert!(answer["result"].get("changes").is_none(), "{answer}");
    }

    for (id, scope, cursor) in [
        (5, project_scope.clone(), project_cursor.clone()),
        (6, task_scope.clone(), task_cursor),
    ] {
        let answer = ask(
            &mut wire,
            &intake,
            &key,
            id,
            "sync.head",
            json!({"scope":scope,"cursor":cursor}),
        )
        .await;
        assert_eq!(answer["ok"], true, "{answer}");
        assert_eq!(answer["result"]["status"], "resync_required", "{answer}");
        assert_eq!(answer["result"]["reason"], "scope_removed", "{answer}");
    }

    let answer = ask(
        &mut wire,
        &intake,
        &key,
        7,
        "sync.snapshot",
        json!({"scope":project_scope.clone(),"page":project_page,"limit":1}),
    )
    .await;
    assert_eq!(answer["ok"], true, "{answer}");
    assert_eq!(answer["result"]["status"], "resync_required", "{answer}");
    assert_eq!(answer["result"]["reason"], "scope_removed", "{answer}");
    assert!(answer["result"].get("records").is_none(), "{answer}");

    let mut forged = project_cursor.into_bytes();
    let last = forged.last_mut().unwrap();
    *last = if *last == b'A' { b'B' } else { b'A' };
    let forged = String::from_utf8(forged).unwrap();
    let answer = ask(
        &mut wire,
        &intake,
        &key,
        8,
        "sync.changes",
        json!({"scope":project_scope.clone(),"cursor":forged}),
    )
    .await;
    assert_eq!(answer["ok"], false, "{answer}");
    assert_eq!(answer["error_code"], "not_found", "{answer}");

    let answer = ask(
        &mut wire,
        &intake,
        &key,
        9,
        "sync.snapshot",
        json!({"scope":project_scope}),
    )
    .await;
    assert_eq!(answer["ok"], false, "{answer}");
    assert_eq!(answer["error_code"], "not_found", "{answer}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn removed_project_scope_authenticates_after_an_ordinary_restart() {
    let dir = tempfile::tempdir().unwrap();
    let (_repo, mut app, project_id) = tracked(dir.path());
    filed(&mut app, &project_id, "restart one");
    filed(&mut app, &project_id, "restart two");
    let state = app.shared();
    let transport = crate::transport::generate_transport_keypair();
    let intake = FrameIntake::new(AppState::handler(Arc::clone(&state)), transport.clone());
    let mut wire = ChannelWire::open();
    let key = crate::transport::generate_session_key();
    wire.open_session(
        &intake,
        "before-restart",
        &session_init("before-restart", &transport.public_key_b64, &key),
    )
    .unwrap();
    wire.outbound.try_recv().unwrap();
    wire.accept(
        &intake,
        client_request(
            &key,
            "before-restart",
            "data",
            json!({"id":1,"method":"sync.snapshot","params":{"scope":{"kind":"project_tasks","project_id":project_id},"limit":1}}),
        ),
    )
    .await
    .unwrap();
    let snapshot = final_answer(&mut wire, &key).await;
    let cursor = snapshot["result"]["cursor"].as_str().unwrap().to_owned();
    let page = snapshot["result"]["next_page"].as_str().unwrap().to_owned();
    {
        let mut app = state.lock().unwrap();
        let path = app.tracker_project_path(&project_id).unwrap();
        app.store
            .as_ref()
            .unwrap()
            .delete_tracker_tasks_of_project(&path, |_| Ok(()))
            .unwrap();
        app.projects.remove(&project_id);
        app.publish_read_metadata();
    }
    drop(wire);
    drop(intake);
    drop(state);

    let restarted = rooted(dir.path())
        .with_task_store(dir.path().join("store"))
        .unwrap();
    let restarted = restarted.shared();
    let transport = crate::transport::generate_transport_keypair();
    let intake = FrameIntake::new(AppState::handler(restarted), transport.clone());
    let mut wire = ChannelWire::open();
    let key = crate::transport::generate_session_key();
    wire.open_session(
        &intake,
        "after-restart",
        &session_init("after-restart", &transport.public_key_b64, &key),
    )
    .unwrap();
    wire.outbound.try_recv().unwrap();
    for (id, method, extra) in [
        (2, "sync.head", json!({"cursor":cursor})),
        (3, "sync.changes", json!({"cursor":cursor})),
        (4, "sync.snapshot", json!({"page":page,"limit":1})),
    ] {
        let mut params = json!({"scope":{"kind":"project_tasks","project_id":project_id}});
        params
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        wire.accept(
            &intake,
            client_request(
                &key,
                "after-restart",
                "data",
                json!({"id":id,"method":method,"params":params}),
            ),
        )
        .await
        .unwrap();
        let answer = final_answer(&mut wire, &key).await;
        assert_eq!(answer["result"]["reason"], "scope_removed", "{answer}");
    }

    let foreign_dir = tempfile::tempdir().unwrap();
    let (_repo, mut foreign_app, foreign_project) = tracked(foreign_dir.path());
    assert_eq!(foreign_project, project_id);
    filed(&mut foreign_app, &foreign_project, "foreign");
    let foreign_state = foreign_app.shared();
    let foreign_transport = crate::transport::generate_transport_keypair();
    let foreign_intake =
        FrameIntake::new(AppState::handler(foreign_state), foreign_transport.clone());
    let mut foreign_wire = ChannelWire::open();
    let foreign_key = crate::transport::generate_session_key();
    foreign_wire
        .open_session(
            &foreign_intake,
            "foreign",
            &session_init("foreign", &foreign_transport.public_key_b64, &foreign_key),
        )
        .unwrap();
    foreign_wire.outbound.try_recv().unwrap();
    foreign_wire
        .accept(
            &foreign_intake,
            client_request(
                &foreign_key,
                "foreign",
                "data",
                json!({"id":1,"method":"sync.snapshot","params":{"scope":{"kind":"project_tasks","project_id":foreign_project}}}),
            ),
        )
        .await
        .unwrap();
    let foreign_cursor = final_answer(&mut foreign_wire, &foreign_key).await["result"]["cursor"]
        .as_str()
        .unwrap()
        .to_owned();
    for (id, token, expected_ok, expected_reason) in [
        (5, foreign_cursor, true, "incarnation_changed"),
        (6, format!("{cursor}A"), false, ""),
    ] {
        wire.accept(
            &intake,
            client_request(
                &key,
                "after-restart",
                "data",
                json!({"id":id,"method":"sync.head","params":{"scope":{"kind":"project_tasks","project_id":project_id},"cursor":token}}),
            ),
        )
        .await
        .unwrap();
        let answer = final_answer(&mut wire, &key).await;
        assert_eq!(answer["ok"], expected_ok, "{answer}");
        if expected_ok {
            assert_eq!(answer["result"]["reason"], expected_reason, "{answer}");
        } else {
            assert_eq!(answer["error_code"], "not_found", "{answer}");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rebound_project_id_resets_positions_without_exposing_the_old_path() {
    let dir = tempfile::tempdir().unwrap();
    let (_old_repo, mut app, project_id) = tracked(dir.path());
    filed(&mut app, &project_id, "old project one");
    filed(&mut app, &project_id, "old project two");
    let old_state = app.shared();
    let transport = crate::transport::generate_transport_keypair();
    let intake = FrameIntake::new(AppState::handler(Arc::clone(&old_state)), transport.clone());
    let mut wire = ChannelWire::open();
    let key = crate::transport::generate_session_key();
    wire.open_session(
        &intake,
        "before-rebind",
        &session_init("before-rebind", &transport.public_key_b64, &key),
    )
    .unwrap();
    wire.outbound.try_recv().unwrap();
    wire.accept(
        &intake,
        client_request(
            &key,
            "before-rebind",
            "data",
            json!({"id":1,"method":"sync.snapshot","params":{"scope":{"kind":"project_tasks","project_id":project_id},"limit":1}}),
        ),
    )
    .await
    .unwrap();
    let snapshot = final_answer(&mut wire, &key).await;
    let cursor = snapshot["result"]["cursor"].as_str().unwrap().to_owned();
    let page = snapshot["result"]["next_page"]
        .as_str()
        .expect("two records produce a continuation")
        .to_owned();
    drop(wire);
    drop(intake);
    drop(old_state);

    let replacement = std::fs::canonicalize(crate::git_fixture::init_repo_named(
        dir.path(),
        "replacement",
    ))
    .unwrap();
    let mut app = rooted(dir.path())
        .with_task_store(dir.path().join("store"))
        .unwrap();
    let rebound_id = added_project(&mut app, &replacement);
    assert_eq!(rebound_id, project_id);
    let replacement_task = filed(&mut app, &rebound_id, "replacement project");
    let state = app.shared();
    let transport = crate::transport::generate_transport_keypair();
    let intake = FrameIntake::new(AppState::handler(state), transport.clone());
    let mut wire = ChannelWire::open();
    let key = crate::transport::generate_session_key();
    wire.open_session(
        &intake,
        "after-rebind",
        &session_init("after-rebind", &transport.public_key_b64, &key),
    )
    .unwrap();
    wire.outbound.try_recv().unwrap();

    for (id, method, extra) in [
        (2, "sync.head", json!({"cursor":cursor})),
        (3, "sync.changes", json!({"cursor":cursor})),
        (4, "sync.snapshot", json!({"page":page,"limit":1})),
    ] {
        let mut params = json!({"scope":{"kind":"project_tasks","project_id":rebound_id}});
        params
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        wire.accept(
            &intake,
            client_request(
                &key,
                "after-rebind",
                "data",
                json!({"id":id,"method":method,"params":params}),
            ),
        )
        .await
        .unwrap();
        let answer = final_answer(&mut wire, &key).await;
        assert_eq!(answer["ok"], true, "{answer}");
        assert_eq!(answer["result"]["status"], "resync_required", "{answer}");
        assert_eq!(answer["result"]["reason"], "scope_removed", "{answer}");
        assert!(answer["result"].get("records").is_none(), "{answer}");
        assert!(answer["result"].get("changes").is_none(), "{answer}");
    }

    wire.accept(
        &intake,
        client_request(
            &key,
            "after-rebind",
            "data",
            json!({"id":5,"method":"sync.snapshot","params":{"scope":{"kind":"project_tasks","project_id":rebound_id}}}),
        ),
    )
    .await
    .unwrap();
    let fresh = final_answer(&mut wire, &key).await;
    assert_eq!(fresh["ok"], true, "{fresh}");
    assert_eq!(fresh["result"]["status"], "ok", "{fresh}");
    assert_eq!(fresh["result"]["records"].as_array().unwrap().len(), 1);
    assert_eq!(fresh["result"]["records"][0]["key"], replacement_task["id"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn task_sync_admission_uses_indexed_authority_without_decoding_the_canonical_record() {
    let dir = tempfile::tempdir().unwrap();
    let (_repo, mut app, project_id) = tracked(dir.path());
    let task = filed(&mut app, &project_id, "indexed authority");
    let task_id = task["id"].as_str().unwrap().to_owned();
    let store = app.store.as_ref().unwrap().clone();
    let state = app.shared();
    let transport = crate::transport::generate_transport_keypair();
    let intake = FrameIntake::new(AppState::handler(state), transport.clone());
    store
        .writer_for_test()
        .execute(
            "UPDATE tracker_tasks SET record='not-json' WHERE id=?1",
            [&task_id],
        )
        .unwrap();
    let mut wire = ChannelWire::open();
    let key = crate::transport::generate_session_key();
    wire.open_session(
        &intake,
        "indexed-sync",
        &session_init("indexed-sync", &transport.public_key_b64, &key),
    )
    .unwrap();
    wire.outbound.try_recv().unwrap();
    wire.accept(
        &intake,
        client_request(
            &key,
            "indexed-sync",
            "data",
            json!({
                "id":1,"method":"sync.snapshot",
                "params":{"scope":{"kind":"task","task_id":task_id}}
            }),
        ),
    )
    .await
    .unwrap();
    let answer = tokio::time::timeout(Duration::from_secs(5), final_answer(&mut wire, &key))
        .await
        .unwrap();
    assert_eq!(answer["ok"], true, "{answer}");
    assert_eq!(
        answer["result"]["records"][0]["row"]["task"]["title"],
        "indexed authority"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn task_sync_admission_fails_closed_on_incomplete_or_unregistered_authority() {
    let dir = tempfile::tempdir().unwrap();
    let (_repo, mut app, project_id) = tracked(dir.path());
    let missing = filed(&mut app, &project_id, "missing summary")["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mismatch = filed(&mut app, &project_id, "mismatched summary")["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let outside = filed(&mut app, &project_id, "outside registry")["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let store = app.store.as_ref().unwrap().clone();
    let state = app.shared();
    let transport = crate::transport::generate_transport_keypair();
    let intake = FrameIntake::new(AppState::handler(state), transport.clone());
    {
        let connection = store.writer_for_test();
        connection
            .execute(
                "DELETE FROM tracker_task_summaries WHERE task_id=?1",
                [&missing],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE tracker_task_summaries SET project_key='/stale' WHERE task_id=?1",
                [&mismatch],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE tracker_tasks SET project_key='/outside' WHERE id=?1",
                [&outside],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE tracker_task_summaries SET project_key='/outside' WHERE task_id=?1",
                [&outside],
            )
            .unwrap();
    }
    let mut wire = ChannelWire::open();
    let key = crate::transport::generate_session_key();
    wire.open_session(
        &intake,
        "authority-sync",
        &session_init("authority-sync", &transport.public_key_b64, &key),
    )
    .unwrap();
    wire.outbound.try_recv().unwrap();
    for (id, task_id) in [missing, mismatch].into_iter().enumerate() {
        wire.accept(
            &intake,
            client_request(
                &key,
                "authority-sync",
                "data",
                json!({"id":id,"method":"sync.snapshot","params":{"scope":{"kind":"task","task_id":task_id}}}),
            ),
        )
        .await
        .unwrap();
        let answer = final_answer(&mut wire, &key).await;
        assert_eq!(answer["ok"], false, "{answer}");
        assert_eq!(answer["error_code"], "unavailable", "{answer}");
        assert_eq!(answer["retryable"], true, "{answer}");
    }
    wire.accept(
        &intake,
        client_request(
            &key,
            "authority-sync",
            "data",
            json!({"id":3,"method":"sync.snapshot","params":{"scope":{"kind":"task","task_id":outside}}}),
        ),
    )
    .await
    .unwrap();
    let answer = final_answer(&mut wire, &key).await;
    assert_eq!(answer["error_code"], "not_found", "{answer}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unregistered_task_project_classifies_authenticated_positions_as_removed() {
    let dir = tempfile::tempdir().unwrap();
    let (_repo, mut app, project_id) = tracked(dir.path());
    let task_id = filed(&mut app, &project_id, "leaves the registry")["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let store = app.store.as_ref().unwrap().clone();
    let state = app.shared();
    let transport = crate::transport::generate_transport_keypair();
    let intake = FrameIntake::new(AppState::handler(state), transport.clone());
    let mut wire = ChannelWire::open();
    let key = crate::transport::generate_session_key();
    wire.open_session(
        &intake,
        "unregistered-task",
        &session_init("unregistered-task", &transport.public_key_b64, &key),
    )
    .unwrap();
    wire.outbound.try_recv().unwrap();
    wire.accept(
        &intake,
        client_request(
            &key,
            "unregistered-task",
            "data",
            json!({"id":1,"method":"sync.snapshot","params":{"scope":{"kind":"task","task_id":task_id}}}),
        ),
    )
    .await
    .unwrap();
    let cursor = final_answer(&mut wire, &key).await["result"]["cursor"]
        .as_str()
        .unwrap()
        .to_owned();
    {
        let connection = store.writer_for_test();
        connection
            .execute(
                "UPDATE tracker_tasks SET project_key='/outside' WHERE id=?1",
                [&task_id],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE tracker_task_summaries SET project_key='/outside' WHERE task_id=?1",
                [&task_id],
            )
            .unwrap();
    }

    for (id, token, expected_ok) in [(2, cursor.clone(), true), (3, format!("{cursor}A"), false)] {
        wire.accept(
            &intake,
            client_request(
                &key,
                "unregistered-task",
                "data",
                json!({"id":id,"method":"sync.changes","params":{"scope":{"kind":"task","task_id":task_id},"cursor":token}}),
            ),
        )
        .await
        .unwrap();
        let answer = final_answer(&mut wire, &key).await;
        assert_eq!(answer["ok"], expected_ok, "{answer}");
        if expected_ok {
            assert_eq!(answer["result"]["status"], "resync_required", "{answer}");
            assert_eq!(answer["result"]["reason"], "scope_removed", "{answer}");
            assert!(answer["result"].get("changes").is_none(), "{answer}");
        } else {
            assert_eq!(answer["error_code"], "not_found", "{answer}");
        }
    }

    wire.accept(
        &intake,
        client_request(
            &key,
            "unregistered-task",
            "data",
            json!({"id":4,"method":"sync.snapshot","params":{"scope":{"kind":"task","task_id":task_id}}}),
        ),
    )
    .await
    .unwrap();
    let answer = final_answer(&mut wire, &key).await;
    assert_eq!(answer["ok"], false, "{answer}");
    assert_eq!(answer["error_code"], "not_found", "{answer}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restored_older_store_resets_a_missing_task_scope_by_incarnation() {
    let dir = tempfile::tempdir().unwrap();
    let backup = dir.path().join("before-task.db");
    let (home, mut app, project_id) = tracked(dir.path());
    app.store.as_ref().unwrap().backup_to(&backup).unwrap();
    let task = filed(&mut app, &project_id, "newer than backup");
    let task_id = task["id"].as_str().unwrap().to_owned();
    let old_cursor = {
        let state = app.shared();
        let transport = crate::transport::generate_transport_keypair();
        let intake = FrameIntake::new(AppState::handler(state), transport.clone());
        let mut wire = ChannelWire::open();
        let key = crate::transport::generate_session_key();
        wire.open_session(
            &intake,
            "pre-restore-sync",
            &session_init("pre-restore-sync", &transport.public_key_b64, &key),
        )
        .unwrap();
        wire.outbound.try_recv().unwrap();
        wire.accept(
            &intake,
            client_request(
                &key,
                "pre-restore-sync",
                "data",
                json!({
                    "id":1,"method":"sync.snapshot",
                    "params":{"scope":{"kind":"task","task_id":task_id}}
                }),
            ),
        )
        .await
        .unwrap();
        let answer = final_answer(&mut wire, &key).await;
        answer["result"]["cursor"].as_str().unwrap().to_owned()
    };

    std::fs::copy(&backup, dir.path().join("store/build.db")).unwrap();
    drop(crate::store::Store::new_after_restore(dir.path().join("store")).unwrap());
    let (_new_home, restored, restored_project) = tracked(dir.path());
    assert_eq!(restored_project, project_id);
    let state = restored.shared();
    let transport = crate::transport::generate_transport_keypair();
    let intake = FrameIntake::new(AppState::handler(state), transport.clone());
    let mut wire = ChannelWire::open();
    let key = crate::transport::generate_session_key();
    wire.open_session(
        &intake,
        "post-restore-sync",
        &session_init("post-restore-sync", &transport.public_key_b64, &key),
    )
    .unwrap();
    wire.outbound.try_recv().unwrap();
    wire.accept(
        &intake,
        client_request(
            &key,
            "post-restore-sync",
            "data",
            json!({
                "id":2,"method":"sync.changes",
                "params":{"scope":{"kind":"task","task_id":task_id},"cursor":old_cursor}
            }),
        ),
    )
    .await
    .unwrap();
    let answer = tokio::time::timeout(Duration::from_secs(5), final_answer(&mut wire, &key))
        .await
        .unwrap();
    assert_eq!(answer["ok"], true, "{answer}");
    assert_eq!(answer["result"]["status"], "resync_required", "{answer}");
    assert_eq!(
        answer["result"]["reason"], "incarnation_changed",
        "{answer}"
    );
    drop(home);
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
