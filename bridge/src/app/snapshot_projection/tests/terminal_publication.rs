use super::*;
use crate::carrier::SessionSender;
use std::os::unix::fs::PermissionsExt;

#[tokio::test]
async fn terminal_creation_pushes_open_and_close_before_observations_are_ready() {
    assert_unpublished_terminal_pushes(false).await;
}

#[tokio::test]
async fn terminal_creation_pushes_open_and_close_for_a_new_checkout_alias() {
    assert_unpublished_terminal_pushes(true).await;
}

async fn assert_unpublished_terminal_pushes(new_alias: bool) {
    let directory = tempfile::tempdir().unwrap();
    let repo = crate::git_fixture::init_repo_named(directory.path(), "repo");
    let mut app = fixture(directory.path()).with_change_window(std::time::Duration::ZERO);
    let project = app.add_project(repo.clone(), "main".into());
    let shell = directory.path().join("terminal-fixture-shell");
    std::fs::write(&shell, "#!/bin/sh\nwhile IFS= read -r line; do :; done\n").unwrap();
    std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o700)).unwrap();
    app.term_shell = shell.display().to_string();
    let (entity, selector) = if new_alias {
        app.initialize_snapshot_projection();
        let alias = "run-new-terminal-alias";
        let active = crate::orchestrator::ActiveRun::workspace_conversation(
            crate::run::RunId::new(alias),
            "New checkout alias".into(),
            repo,
            Default::default(),
        );
        app.projects.bind_entity(alias.into(), project);
        app.runs.insert(alias.into(), active);
        (alias.to_owned(), json!({"run_id":alias}))
    } else {
        assert!(!app.snapshot_observations.ready);
        (project.clone(), json!({"project_id":project}))
    };
    let state = Arc::new(Mutex::new(app));
    let (bus, clock) = {
        let mut app = state.lock().unwrap();
        app.self_handle = Some(Arc::downgrade(&state));
        assert!(app
            .facts_handle
            .set(crate::app::facts::FactsSource::new(
                app.read_view.clone(),
                Arc::downgrade(&state),
            ))
            .is_ok());
        (app.changes(), Arc::clone(&app.frame_clock))
    };
    let (sender, mut received, key) = SessionSender::observable("terminal-new-subject");
    bus.subscribe(
        &sender,
        serde_json::from_value(json!({
            "subscription_id":"terminal-new-subject", "scope":{"kind":"entity","id":entity},
            "kinds":["terminals"], "mode":"realtime",
        }))
        .unwrap(),
    );
    let created = {
        let timer = clock.frame("term.create");
        crate::app::runtime::terminals::term_create(&state, &selector, &timer).unwrap()
    };
    bus.flush();
    let opened = drain_terminal_rows(&mut received, &key, &entity);
    let child = {
        let app = state.lock().unwrap();
        let key = app
            .tab_key_of_wire_id(created["term_id"].as_str().unwrap())
            .unwrap();
        Arc::clone(&app.session_registry.test_tab(&key).unwrap().session)
    };
    let closed = state
        .lock()
        .unwrap()
        .term_close(&json!({"term_id":created["term_id"]}));
    child.end();
    let reaped = child.exited_within(std::time::Duration::from_secs(2));
    bus.flush();
    let closed_rows = drain_terminal_rows(&mut received, &key, &entity);
    // Cleanup precedes assertions so a red push regression cannot leak a PTY.
    assert!(reaped);
    assert!(closed.is_ok(), "{closed:?}");
    assert_eq!(
        opened
            .last()
            .and_then(|row| row["tabs"].as_array())
            .map(Vec::len),
        Some(1),
        "a checkout outside the accepted view must push its opened terminal: {opened:?}"
    );
    assert_eq!(
        closed_rows.last().map(|row| &row["tabs"]),
        Some(&json!([])),
        "a checkout outside the accepted view must push its closed terminal: {closed_rows:?}"
    );
}

fn drain_terminal_rows(
    received: &mut tokio::sync::mpsc::UnboundedReceiver<crate::carrier::OutboundEnvelope>,
    key: &str,
    entity: &str,
) -> Vec<Value> {
    let mut rows = Vec::new();
    while let Ok(frame) = received.try_recv() {
        let frame = SessionSender::decrypt_push(key, &frame);
        rows.extend(
            frame["items"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|item| item["entity_id"] == entity)
                .filter_map(|item| item.get("terminals").cloned()),
        );
    }
    rows
}
