use super::*;

#[tokio::test]
async fn failed_conversation_compaction_save_is_refused_without_publication_or_push() {
    let (directory, repository) = init_repo();
    let (state, handler, sender, mut incoming, key) =
        push::greeted_push_session(&repository, directory.path());
    push::watch_everything(&handler, &sender);
    let owners = {
        let mut app = state.lock().unwrap();
        let plan = file_legacy_task(&mut app, "committed policy plan");
        let run = adopted_run(
            &mut app,
            &repository,
            directory.path(),
            "committed-policy-run",
        );
        [plan, run]
    };
    push::settled_pushes(&mut incoming, &key).await;
    for owner in owners {
        let mut app = state.lock().unwrap();
        let agent = app
            .entity_agents(&owner)
            .unwrap()
            .iter()
            .next()
            .unwrap()
            .id
            .clone();
        let before = app.read_view.load();
        app.store.as_ref().unwrap().fail_next_write();
        let response = app.set_conversation_context_limit(&owner, &agent, Some(123_456));
        assert!(
            response.is_err(),
            "a failed save was acknowledged: {response:?}"
        );
        assert!(Arc::ptr_eq(&before, &app.read_view.load()));
        assert_eq!(
            app.entity_agents(&owner)
                .unwrap()
                .by_id(&agent)
                .unwrap()
                .max_context_tokens,
            None
        );
    }
    let pushed = push::settled_pushes(&mut incoming, &key).await;
    assert!(push::changed_entities(&pushed).is_empty(), "{pushed:?}");
    assert!(
        pushed
            .iter()
            .all(|frame| frame["type"] != "compaction.changed"),
        "{pushed:?}"
    );
}

#[test]
fn failed_settings_persistence_cannot_leak_through_a_later_publication() {
    let directory = tempfile::tempdir().unwrap();
    let state = AppState::new_unrooted(
        directory.path().join("worktrees"),
        "main",
        true,
        directory.path().join("unused.sock").to_str().unwrap(),
    )
    .with_config(directory.path().join("config.json"))
    .unwrap()
    .shared();
    let mut app = state.lock().unwrap();
    for failure in [ConfigPersistStep::Write, ConfigPersistStep::Rename] {
        let before = app.read_view.load();
        app.config_persist_failure = Some(failure);
        assert!(app
            .settings_set(&json!({
                "watch_agent_filed_tasks": true,
                "compact_above_tokens": 123_456,
            }))
            .is_err());
        assert!(Arc::ptr_eq(&before, &app.read_view.load()));
        assert!(!app.watch_agent_filed_tasks);
        app.config_persist_failure = None;
        app.settings_set(&json!({"workspace_idle_secs": 1234}))
            .unwrap();
        let after = app.read_view.load();
        let settings = after.get("settings.get", "").unwrap();
        assert_eq!(settings["watch_agent_filed_tasks"], false);
        assert_eq!(
            settings["compact_above_tokens"],
            before.get("settings.get", "").unwrap()["compact_above_tokens"]
        );
        assert_eq!(settings["workspace_idle_secs"], 1234);
    }
}

#[test]
fn failed_compaction_persistence_cannot_leak_through_a_later_publication() {
    let directory = tempfile::tempdir().unwrap();
    let state = AppState::new_unrooted(
        directory.path().join("worktrees"),
        "main",
        true,
        directory.path().join("unused.sock").to_str().unwrap(),
    )
    .with_config(directory.path().join("config.json"))
    .unwrap()
    .shared();
    let mut app = state.lock().unwrap();
    for failure in [ConfigPersistStep::Write, ConfigPersistStep::Rename] {
        let before = app.read_view.load();
        app.config_persist_failure = Some(failure);
        assert!(app
            .patch_compaction_thresholds(
                crate::compaction::CompactionScope::Device,
                &json!({"claude": 123_456}),
            )
            .is_err());
        assert!(Arc::ptr_eq(&before, &app.read_view.load()));
        app.config_persist_failure = None;
        app.settings_set(&json!({"watch_agent_filed_tasks": true}))
            .unwrap();
        let after = app.read_view.load();
        assert_eq!(
            after.get("settings.get", "").unwrap()["compaction_thresholds"],
            before.get("settings.get", "").unwrap()["compaction_thresholds"],
        );
    }
}
