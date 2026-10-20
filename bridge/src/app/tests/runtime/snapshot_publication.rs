use super::*;

#[test]
fn successful_settings_reply_publishes_the_value_read_by_other_clients() {
    let directory = tempfile::tempdir().unwrap();
    let app = AppState::new_unrooted(
        directory.path().join("worktrees"),
        "main",
        true,
        directory.path().join("unused.sock").to_str().unwrap(),
    )
    .with_config(directory.path().join("config.json"))
    .unwrap();
    let state = app.shared();
    let handler = AppState::handler(Arc::clone(&state));
    let reader = SessionSender::detached("snapshot-reader");
    let writer = SessionSender::detached("snapshot-writer");
    let before = handler.call(reader.clone(), req("settings.get", json!({})));
    assert_eq!(before["ok"], true, "{before}");
    assert_eq!(before["result"]["watch_agent_filed_tasks"], false);

    let reply = handler.call(
        writer,
        req("settings.set", json!({"watch_agent_filed_tasks": true})),
    );
    assert_eq!(reply["ok"], true, "{reply}");
    let after = handler.call(reader, req("settings.get", json!({})));
    assert_eq!(after["result"]["watch_agent_filed_tasks"], true, "{after}");
    assert_eq!(
        state.lock().unwrap().read_view.load().get("settings.get", "").unwrap()
            ["watch_agent_filed_tasks"],
        true,
        "the acknowledged value belongs to the immutable published root"
    );
}

#[test]
fn project_compaction_publication_updates_existing_agent_observations() {
    let (directory, repository) = init_repo();
    let mut app = qa_state(&repository, directory.path());
    let owner = adopted_run(&mut app, &repository, directory.path(), "snapshot-policy");
    let project = app.projects.project_id_of(&owner).unwrap().to_owned();
    let agent = app.runs[&owner].agents.iter().next().unwrap();
    let provider = agent.choice.provider.wire_id().to_owned();
    let state = app.shared();
    let handler = AppState::handler(Arc::clone(&state));
    {
        let mut app = state.lock().unwrap();
        app.patch_compaction_thresholds(
            crate::compaction::CompactionScope::Project(project),
            &json!({provider: 151_001}),
        )
        .unwrap();
    }
    let after = handler.call(
        SessionSender::detached("snapshot-policy-reader"),
        req("agent.list", json!({"entity_id": owner})),
    );
    assert_eq!(after["ok"], true, "{after}");
    assert_eq!(after["result"]["agents"][0]["compaction"]["configured"]["tokens"], 151_001, "{after}");
}
